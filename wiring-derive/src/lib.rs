use syn::{parse_quote, GenericParam, Generics, Meta, TypeParamBound};

fn has_type_params(generics: &Generics) -> bool {
    generics
        .params
        .iter()
        .any(|param| matches!(param, GenericParam::Type(_)))
}

#[proc_macro_derive(Wiring, attributes(tag, fixed))]
pub fn wiring_proc_macro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    use quote::quote;
    use syn::{parse_macro_input, Data, DeriveInput, Fields, Index, Variant};

    let input = parse_macro_input!(input as DeriveInput);
    let this = input.ident;
    let data = input.data;

    let mut g = input.generics.clone();
    let attrs = input.attrs;
    if has_type_params(&g) {
        let params = g.type_params().cloned().collect::<Vec<_>>();
        let where_clause = g.make_where_clause();
        for param in params {
            let ident = &param.ident;
            let predicate: TypeParamBound = parse_quote!(Wiring);
            where_clause.predicates.push(parse_quote!(#ident: #predicate));
        }
    }
    let mut fixed_size_tokens = proc_macro2::TokenStream::new();

    match data {
        Data::Struct(data) => match data.fields {
            syn::Fields::Named(fields) => {
                let mut wiring_calls = Vec::new();
                let mut wiring_ref_calls = Vec::new();
                let mut sync_wiring_calls = Vec::new();

                let mut concat_calls = Vec::new();

                let mixed_tokens = fields.named.iter().map(|f| {
                    let t = &f.ty;
                    quote::quote! { <#t as Wiring>::MIXED }
                });
                let mixed_check: proc_macro2::TokenStream = quote::quote! {
                    false #(|| #mixed_tokens)*
                };
                let mut wiring_concat_fields_calls = Vec::new();
                let mut ref_concat_fields_calls = Vec::new();
                let mut sync_concat_fields_calls = Vec::new();
                let mut concat_fixed_size_tokens = proc_macro2::TokenStream::new();
                let fields_len = fields.named.len();

                let mut field_count = 0;
                let mut concat_start: Option<usize> = None;

                for field in fields.named.iter() {
                    field_count += 1;
                    let field_name = &field.ident;

                    let mut is_concat_start = false;
                    let mut is_concat_mid = false;
                    let mut is_concat_end = false;

                    let field_type = field.ty.clone();
                    // Generate tokens for each field's FIXED_SIZE

                    let tokens = quote! {
                        + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                    };
                    fixed_size_tokens.extend(tokens);

                    field.attrs.iter().for_each(|attr| {
                        // Here you check for the specific attribute that indicates the existence of a helper macro
                        if attr.path().is_ident("fixed") {
                            if let Meta::List(l) = attr.meta.clone() {
                                if concat_start.is_some() {
                                    panic!("Confilict in the fixed range")
                                }
                                if fields_len == 1 {
                                    panic!("cannot apply fixed on struct with less than 2 fields");
                                }
                                let t = l.tokens.to_string().parse::<usize>().unwrap();
                                if t == 1 {
                                    panic!("cannot apply fixed with less than 2 fields");
                                }

                                if t > fields_len - (field_count - 1) {
                                    panic!("cannot fixed beyond the struct length");
                                }
                                concat_start.replace(t);
                                // now this is new start, so we set start flag, but we must reset it to false.
                                is_concat_start = true;
                            } else {
                                if concat_start.is_some() {
                                    panic!("Confilict in the fixed range")
                                }
                                let t = fields_len - (field_count - 1);
                                if t == 1 {
                                    panic!("cannot apply fixed with less than 2 fields");
                                }
                                concat_start.replace(t);
                                is_concat_start = true;
                            }
                        }
                    });

                    let q = quote! {
                        let (left, buf) = buf.split_at_mut(<#field_type as wiring::prelude::Wiring>::FIXED_SIZE);
                        self.#field_name.concat_array(left);
                    };
                    concat_calls.push(q);

                    if let Some(concat_s) = concat_start.as_mut() {
                        // if it was zero it means we failed, it must never zero.
                        *concat_s -= 1;

                        if !is_concat_start {
                            if *concat_s == 0 {
                                is_concat_end = true;
                            } else {
                                is_concat_mid = true;
                            }
                        }
                    }

                    if is_concat_end {
                        concat_start.take();
                    }

                    if is_concat_start {
                        let tokens = quote! {
                            <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                        };
                        concat_fixed_size_tokens.extend(tokens);
                    } else if is_concat_mid || is_concat_end {
                        let tokens = quote! {
                           + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                        };
                        concat_fixed_size_tokens.extend(tokens);
                    }

                    // we group them
                    let wiring_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           (&_a_[.._i_]).wiring_ref(wire).await?;
                        }
                    } else {
                        quote! { self.#field_name.wiring(wire).await?; }
                    };

                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        wiring_calls.push(wiring_call);
                    } else {
                        wiring_concat_fields_calls.push(wiring_call);
                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#wiring_concat_fields_calls)*;
                            };
                            wiring_calls.push(q);
                        }
                    }

                    let wiring_ref_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           (&_a_[.._i_]).wiring_ref(wire).await?;
                        }
                    } else {
                        quote! { (&self.#field_name).wiring_ref(wire).await?; }
                    };

                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        wiring_ref_calls.push(wiring_ref_call);
                    } else {
                        ref_concat_fields_calls.push(wiring_ref_call);

                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#ref_concat_fields_calls)*;
                            };
                            wiring_ref_calls.push(q);
                        }
                    }
                    // for concat we dont push unless we concat all of them
                    let sync_wiring_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           wire.sync_wire_all::<true>(&_a_[.._i_])?;
                        }
                    } else {
                        quote! { (&self.#field_name).sync_wiring(wire)?; }
                    };
                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        sync_wiring_calls.push(sync_wiring_call);
                    } else {
                        sync_concat_fields_calls.push(sync_wiring_call);

                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#sync_concat_fields_calls)*
                            };
                            sync_wiring_calls.push(q);
                            // reset for any other concat fields
                            concat_fixed_size_tokens = proc_macro2::TokenStream::new();
                            wiring_concat_fields_calls.clear();
                            ref_concat_fields_calls.clear();
                            sync_concat_fields_calls.clear();
                        }
                    }
                }
                let s = fixed_size_tokens.to_string();
                let s = s.trim();
                let s = s.char_indices().nth(1).map(|(i, _)| &s[i..]).unwrap_or("");
                fixed_size_tokens = s
                    .parse::<proc_macro2::TokenStream>()
                    .expect("Failed to parse back to TokenStream");

                let (impl_g, ty_g, wh_g) = g.split_for_impl();

                let x = quote! {

                    impl #impl_g Wiring for #this #ty_g #wh_g {

                        const FIXED_SIZE: usize = #fixed_size_tokens;
                        const MIXED: bool = #mixed_check;
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(#wiring_calls)*
                                Ok(())
                            }
                        }
                        #[inline]
                        fn wiring_ref<W: wiring::prelude::Wire>(&self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(#wiring_ref_calls)*
                                Ok(())
                            }
                        }
                        #[inline(always)]
                        fn sync_wiring<W: wiring::prelude::Wire + std::io::Write>(&self, wire: &mut W) -> Result<(), std::io::Error> {
                                #(#sync_wiring_calls)*
                                Ok(())
                        }
                        #[inline(always)]
                        fn concat_array(&self, buf: &mut [u8]) {
                            #(#concat_calls)*
                        }
                    }


                };
                return x.into();
            }
            syn::Fields::Unnamed(fields) => {
                let mut wiring_calls = Vec::new();
                let mut wiring_ref_calls = Vec::new();
                let mut sync_wiring_calls = Vec::new();

                let mut concat_calls = Vec::new();

                let mixed_tokens = fields.unnamed.iter().map(|f| {
                    let t = &f.ty;
                    quote::quote! { <#t as Wiring>::MIXED }
                });
                let mixed_check: proc_macro2::TokenStream = quote::quote! {
                    false #(|| #mixed_tokens)*
                };
                let mut wiring_concat_fields_calls = Vec::new();
                let mut ref_concat_fields_calls = Vec::new();
                let mut sync_concat_fields_calls = Vec::new();
                let mut concat_fixed_size_tokens = proc_macro2::TokenStream::new();
                let fields_len = fields.unnamed.len();

                let mut field_count = 0;
                let mut concat_start: Option<usize> = None;

                for (index, field) in fields.unnamed.iter().enumerate() {
                    field_count += 1;
                    let field_name = Index::from(index);

                    let mut is_concat_start = false;
                    let mut is_concat_mid = false;
                    let mut is_concat_end = false;

                    let field_type = field.ty.clone();
                    // Generate tokens for each field's FIXED_SIZE

                    let tokens = quote! {
                        + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                    };
                    fixed_size_tokens.extend(tokens);

                    field.attrs.iter().for_each(|attr| {
                        // Here you check for the specific attribute that indicates the existence of a helper macro
                        if attr.path().is_ident("fixed") {
                            if let Meta::List(l) = attr.meta.clone() {
                                if concat_start.is_some() {
                                    panic!("Confilict in the fixed range")
                                }
                                if fields_len == 1 {
                                    panic!("cannot apply fixed on struct with less than 2 fields");
                                }
                                let t = l.tokens.to_string().parse::<usize>().unwrap();
                                if t == 1 {
                                    panic!("cannot apply fixed with less than 2 fields");
                                }

                                if t > fields_len - (field_count - 1) {
                                    panic!("cannot fixed beyond the struct length");
                                }
                                concat_start.replace(t);
                                // now this is new start, so we set start flag, but we must reset it to false.
                                is_concat_start = true;
                            } else {
                                if concat_start.is_some() {
                                    panic!("Confilict in the fixed range")
                                }
                                let t = fields_len - (field_count - 1);
                                if t == 1 {
                                    panic!("cannot apply fixed with less than 2 fields");
                                }
                                concat_start.replace(t);
                                is_concat_start = true;
                            }
                        }
                    });

                    let q = quote! {
                        let (left, buf) = buf.split_at_mut(<#field_type as wiring::prelude::Wiring>::FIXED_SIZE);
                        self.#field_name.concat_array(left);
                    };
                    concat_calls.push(q);

                    if let Some(concat_s) = concat_start.as_mut() {
                        *concat_s -= 1;

                        if !is_concat_start {
                            if *concat_s == 0 {
                                is_concat_end = true;
                            } else {
                                is_concat_mid = true;
                            }
                        }
                    }

                    if is_concat_end {
                        concat_start.take();
                    }

                    if is_concat_start {
                        let tokens = quote! {
                            <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                        };
                        concat_fixed_size_tokens.extend(tokens);
                    } else if is_concat_mid || is_concat_end {
                        let tokens = quote! {
                           + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE
                        };
                        concat_fixed_size_tokens.extend(tokens);
                    }

                    // we group them
                    let wiring_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           (&_a_[.._i_]).wiring_ref(wire).await?;
                        }
                    } else {
                        quote! { self.#field_name.wiring(wire).await?; }
                    };

                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        wiring_calls.push(wiring_call);
                    } else {
                        wiring_concat_fields_calls.push(wiring_call);
                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#wiring_concat_fields_calls)*;
                            };
                            wiring_calls.push(q);
                        }
                    }

                    let wiring_ref_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           (&_a_[.._i_]).wiring_ref(wire).await?;
                        }
                    } else {
                        quote! { (&self.#field_name).wiring_ref(wire).await?; }
                    };

                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        wiring_ref_calls.push(wiring_ref_call);
                    } else {
                        ref_concat_fields_calls.push(wiring_ref_call);

                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#ref_concat_fields_calls)*;
                            };
                            wiring_ref_calls.push(q);
                        }
                    }
                    // for concat we dont push unless we concat all of them
                    let sync_wiring_call = if is_concat_start {
                        quote! {
                           self.#field_name.concat_array(&mut _a_);
                           let mut _i_ = <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_mid {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                        }
                    } else if is_concat_end {
                        quote! {
                           self.#field_name.concat_array(&mut _a_[_i_.._i_ + <#field_type as wiring::prelude::Wiring>::FIXED_SIZE]);
                           _i_ += <#field_type as wiring::prelude::Wiring>::FIXED_SIZE;
                           wire.sync_wire_all::<true>(&_a_[.._i_])?;
                        }
                    } else {
                        quote! { (&self.#field_name).sync_wiring(wire)?; }
                    };
                    if !is_concat_start && !is_concat_mid && !is_concat_end {
                        sync_wiring_calls.push(sync_wiring_call);
                    } else {
                        sync_concat_fields_calls.push(sync_wiring_call);

                        // check if is the last call to build the buffer and push it as call
                        if is_concat_end {
                            let q = quote! {
                               let mut _a_ = [0u8; #concat_fixed_size_tokens];
                               #(#sync_concat_fields_calls)*
                            };
                            sync_wiring_calls.push(q);
                            // reset for any other concat fields
                            concat_fixed_size_tokens = proc_macro2::TokenStream::new();
                            wiring_concat_fields_calls.clear();
                            ref_concat_fields_calls.clear();
                            sync_concat_fields_calls.clear();
                        }
                    }
                }
                let s = fixed_size_tokens.to_string();
                let s = s.trim();
                let s = s.char_indices().nth(1).map(|(i, _)| &s[i..]).unwrap_or("");
                fixed_size_tokens = s
                    .parse::<proc_macro2::TokenStream>()
                    .expect("Failed to parse back to TokenStream");

                let (impl_g, ty_g, wh_g) = g.split_for_impl();

                let x = quote! {

                    impl #impl_g Wiring for #this #ty_g #wh_g {

                        const FIXED_SIZE: usize = #fixed_size_tokens;
                        const MIXED: bool = #mixed_check;
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(#wiring_calls)*
                                Ok(())
                            }
                        }
                        #[inline]
                        fn wiring_ref<W: wiring::prelude::Wire>(&self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(#wiring_ref_calls)*
                                Ok(())
                            }
                        }
                        #[inline(always)]
                        fn sync_wiring<W: wiring::prelude::Wire + std::io::Write>(&self, wire: &mut W) -> Result<(), std::io::Error> {
                                #(#sync_wiring_calls)*
                                Ok(())
                        }
                        #[inline(always)]
                        fn concat_array(&self, buf: &mut [u8]) {
                            #(#concat_calls)*
                        }
                    }


                };
                return x.into();
            }
            syn::Fields::Unit => {
                let expanded = quote::quote! {
                    impl Wiring for #this {
                        const FIXED_SIZE: usize = 1;
                        const MIXED: bool = false;
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                ().wiring(wire).await
                            }
                        }
                        #[inline]
                        fn wiring_ref<W: wiring::prelude::Wire>(&self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                ().wiring_ref(wire).await?;
                                Ok(())
                            }
                        }
                        #[inline]
                        fn sync_wiring<W: wiring::prelude::Wire + std::io::Write>(&self, wire: &mut W) -> Result<(), std::io::Error> {
                                ().sync_wiring(wire)?;
                                Ok(())
                        }
                        #[inline(always)]
                        #[allow(unused)]
                        fn concat_array(&self, buf: &mut [u8]) {
                            buf[0] = 1u8;
                        }
                    }

                };

                return expanded.into();
            }
        },
        Data::Enum(enum_data) => {
            let variants = enum_data.variants;

            let len = variants.len();
            let tag = get_tag(&attrs, len);

            let safe = variants
                .iter()
                .map(|Variant { fields, .. }| {
                    //
                    match fields {
                        Fields::Named(n) => {
                            let n = n
                                .named
                                .iter()
                                .map(|f| {
                                    let t = &f.ty;
                                    quote! {
                                        <#t as Wiring>::SAFE
                                    }
                                })
                                .collect::<Vec<_>>();
                            n
                        }
                        Fields::Unit => {
                            let n = quote! {
                                true
                            };
                            vec![n]
                        }
                        Fields::Unnamed(u) => {
                            let n = u
                                .unnamed
                                .iter()
                                .map(|f| {
                                    let t = &f.ty;
                                    quote! {
                                        <#t as Wiring>::SAFE
                                    }
                                })
                                .collect::<Vec<_>>();
                            n
                        }
                    }
                })
                .collect::<Vec<_>>();

            let safe_1 = safe.clone().into_iter().flatten();

            let cases = variants
                .iter()
                .enumerate()
                .map(|(idx, Variant { ident, fields, .. })| match fields {
                    Fields::Named(named) => {
                        let named_field = named.named.iter().map(|n| &n.ident);
                        let n_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                            #this::#ident { #(#named_field, )* } => {
                                (#idx as #tag).wiring(wire).await?;
                                #(#n_field.wiring(wire).await?;)*
                            }
                        }
                    }
                    Fields::Unnamed(unamed) => {
                        let unamed = &unamed.unnamed;
                        let n_field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });
                        let field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });

                        quote::quote! {
                            #this::#ident ( #(#n_field, )* ) => {
                                (#idx as #tag).wiring(wire).await?;
                                #(#field.wiring(wire).await?;)*
                            }
                        }
                    }
                    Fields::Unit => {
                        quote::quote! {
                            #this::#ident => {
                                (#idx as #tag).wiring(wire).await?;
                            }
                        }
                    }
                });

            let cases_r = variants
                .iter()
                .enumerate()
                .map(|(idx, Variant { ident, fields, .. })| match fields {
                    Fields::Named(named) => {
                        let named_field = named.named.iter().map(|n| &n.ident);
                        let n_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                            #this::#ident { #(#named_field, )* } => {
                                (#idx as #tag).wiring(wire).await?;
                                #(#n_field.wiring_ref(wire).await?;)*
                            }
                        }
                    }
                    Fields::Unnamed(unamed) => {
                        let unamed = &unamed.unnamed;
                        let n_field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });
                        let field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });

                        quote::quote! {
                            #this::#ident ( #(#n_field, )* ) => {
                                (#idx as #tag).wiring(wire).await?;
                                #(#field.wiring_ref(wire).await?;)*
                            }
                        }
                    }
                    Fields::Unit => {
                        quote::quote! {
                            #this::#ident => {
                                (#idx as #tag).wiring(wire).await?;
                            }
                        }
                    }
                });

            let cases_s = variants
                .iter()
                .enumerate()
                .map(|(idx, Variant { ident, fields, .. })| match fields {
                    Fields::Named(named) => {
                        let named_field = named.named.iter().map(|n| &n.ident);
                        let n_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                            #this::#ident { #(#named_field, )* } => {
                                (#idx as #tag).sync_wiring(wire)?;
                                #(#n_field.sync_wiring(wire)?;)*
                            }
                        }
                    }
                    Fields::Unnamed(unamed) => {
                        let unamed = &unamed.unnamed;
                        let n_field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });
                        let field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });

                        quote::quote! {
                            #this::#ident ( #(#n_field, )* ) => {
                                (#idx as #tag).sync_wiring(wire)?;
                                #(#field.sync_wiring(wire)?;)*
                            }
                        }
                    }
                    Fields::Unit => {
                        quote::quote! {
                            #this::#ident => {
                                (#idx as #tag).sync_wiring(wire)?;
                            }
                        }
                    }
                });
            let cases_concat = variants
                .iter()
                .enumerate()
                .map(|(idx, Variant { ident, fields, .. })| match fields {
                    Fields::Named(named) => {
                        let named_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                            #this::#ident { #(#named_field, )* } => {
                                //(#idx as #tag).concat_array(buf)
                            }
                        }
                    }
                    Fields::Unnamed(unamed) => {
                        let unamed = &unamed.unnamed;
                        let n_field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });

                        quote::quote! {
                            #this::#ident ( #(#n_field, )* ) => {
                                //(#idx as #tag).concat_array(buf)
                            }
                        }
                    }
                    Fields::Unit => {
                        quote::quote! {
                            #this::#ident => {
                                (#idx as #tag).concat_array(buf)
                            }
                        }
                    }
                });

            let (impl_g, ty_g, wh_g) = g.split_for_impl();

            let expanded = quote::quote! {

                impl #impl_g Wiring for #this #ty_g #wh_g {
                    const SAFE: bool = true #( && #safe_1)*;
                    const FIXED_SIZE: usize = std::mem::size_of::<#tag>(); // NOTE: currently only Units are supported for concat.
                    #[inline]
                    fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                        async move {
                            match self {
                                #(#cases)*
                            }
                            Ok(())
                        }
                    }
                    #[inline]
                    fn wiring_ref<W: wiring::prelude::Wire>(&self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                        async move {
                            match self {
                                #(#cases_r)*
                            }
                            Ok(())
                        }
                    }
                    #[inline]
                    fn sync_wiring<W: wiring::prelude::Wire + std::io::Write>(&self, wire: &mut W) -> Result<(), std::io::Error> {
                            match self {
                                #(#cases_s)*
                            }
                            Ok(())
                    }
                    #[inline]
                    #[allow(unused)]
                    fn concat_array(&self, buf: &mut [u8]) {
                        match self {
                            #(#cases_concat)*
                        }
                    }
                }


            };

            return expanded.into();
        }
        _ => {
            panic!("Wiring doesn't support union")
        }
    }
}

fn get_tag(attrs: &Vec<syn::Attribute>, len: usize) -> proc_macro2::TokenStream {
    let mut tag = quote::quote! {
        u16
    };

    for a in attrs {
        if a.path().is_ident("tag") {
            let _ = a.parse_nested_meta(|meta| {
                if meta.path.is_ident("u8") {
                    if len > u16::MAX as usize {
                        panic!("The variants are more than u8::MAX, please set #[tag(u16)]")
                    } else {
                        tag = quote::quote! {
                            u8
                        };
                        return Ok(());
                    }
                }
                if meta.path.is_ident("u16") {
                    if len > u16::MAX as usize {
                        panic!("The variants are more than u16::MAX, please set #[tag(u32)]")
                    } else {
                        tag = quote::quote! {
                            u16
                        };
                        return Ok(());
                    }
                }
                if meta.path.is_ident("u32") {
                    if len > u32::MAX as usize {
                        panic!("The variants are more than u32::MAX, please tell me what project are working on")
                    } else {
                        tag = quote::quote! {
                            u32
                        };
                    }
                    return Ok(());
                }
                panic!("Unexpected tag int type, only u8, u16, u32 are supported")
            });
        }
    }
    tag
}

#[proc_macro_derive(Unwiring, attributes(tag))]
pub fn unwiring_proc_macro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    use quote::quote;
    use syn::{parse_macro_input, Data, DeriveInput, Variant};
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let data = input.data;
    let mut g = input.generics.clone();
    let attrs = input.attrs;
    if has_type_params(&g) {
        let params = g.type_params().cloned().collect::<Vec<_>>();
        let where_clause = g.make_where_clause();
        for param in params {
            let ident = &param.ident;
            let predicate: TypeParamBound = parse_quote!(Unwiring);
            where_clause.predicates.push(parse_quote!(#ident: #predicate));
        }
    }
    let (impl_g, ty_g, wh_g) = g.split_for_impl();

    let mut fixed_size_tokens = proc_macro2::TokenStream::new();

    match data {
        Data::Struct(data) => match data.fields {
            syn::Fields::Named(fields) => {
                let named = fields.named;
                let name = named.iter().map(|f| &f.ident);
                let name_s = name.clone();
                let ty = named.iter().map(|f| &f.ty);

                let mixed_tokens = named.iter().map(|f| {
                    let t = &f.ty;
                    quote::quote! { <#t as Unwiring>::MIXED }
                });
                let mixed_check = quote::quote! {
                    false #(|| #mixed_tokens)*
                };

                for field in named.iter() {
                    let field_type = field.ty.clone();
                    let tokens = quote! {
                        + <#field_type as wiring::prelude::Unwiring>::FIXED_SIZE
                    };

                    fixed_size_tokens.extend(tokens);
                }

                let s = fixed_size_tokens.to_string();
                let s = s.trim();
                let s = s.char_indices().nth(1).map(|(i, _)| &s[i..]).unwrap_or("");
                fixed_size_tokens = s
                    .parse::<proc_macro2::TokenStream>()
                    .expect("Failed to parse back to TokenStream");

                let expaned = quote::quote! {

                    impl #impl_g Unwiring for #ident #ty_g #wh_g {
                        const FIXED_SIZE: usize = #fixed_size_tokens;
                        const MIXED: bool = #mixed_check;

                        #[inline]
                        fn unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
                            async move {
                                Ok(
                                    Self {
                                        #(#name: wire.unwiring().await?,)*
                                    }
                                )
                            }
                        }
                        #[inline(always)]
                        fn sync_unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> Result<Self, std::io::Error> where W: std::io::Read {
                             Ok(
                                Self {
                                    #(#name_s: wire.sync_unwiring()?,)*
                                }
                            )
                        }
                        #[inline(always)]
                        fn bytes_length<W: wiring::prelude::Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
                        where
                            W: std::io::Read,
                        {
                            let mut total_bytes_len = 0;
                            for _ in 0..count {

                                #(
                                    total_bytes_len += <#ty as Unwiring>::bytes_length(wire, 1)?;
                                )*

                            }
                            Ok(total_bytes_len)
                        }

                    }

                };

                return expaned.into();
            }
            syn::Fields::Unnamed(fields) => {
                let unamed = fields.unnamed;
                let ty = unamed.iter().map(|i| &i.ty);
                let ty_s = ty.clone();
                let ty_l = ty.clone();
                let mixed_tokens = unamed.iter().map(|f| {
                    let t = &f.ty;
                    quote::quote! { <#t as Unwiring>::MIXED }
                });
                let mixed_check: proc_macro2::TokenStream = quote::quote! {
                    false #(|| #mixed_tokens)*
                };

                for field in unamed.iter() {
                    let field_type = field.ty.clone();
                    let tokens = quote! {
                        + <#field_type as wiring::prelude::Unwiring>::FIXED_SIZE
                    };

                    fixed_size_tokens.extend(tokens);
                }

                let s = fixed_size_tokens.to_string();
                let s = s.trim();
                let s = s.char_indices().nth(1).map(|(i, _)| &s[i..]).unwrap_or("");
                fixed_size_tokens = s
                    .parse::<proc_macro2::TokenStream>()
                    .expect("Failed to parse back to TokenStream");

                let expaned = quote::quote! {

                    impl #impl_g Unwiring for #ident #ty_g #wh_g {
                        const FIXED_SIZE: usize = #fixed_size_tokens;
                        const MIXED: bool = #mixed_check;
                        #[inline]
                        fn unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
                            async move {
                                Ok(
                                    Self (
                                        #(wire.unwiring::<#ty>().await?,)*
                                    )
                                )
                            }
                        }
                        #[inline(always)]
                        fn sync_unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> Result<Self, std::io::Error> where W: std::io::Read {
                             Ok(
                                Self (
                                    #(wire.sync_unwiring::<#ty_s>()?,)*
                                )
                            )
                        }
                        #[inline(always)]
                        fn bytes_length<W: wiring::prelude::Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
                        where
                            W: std::io::Read,
                        {
                            let mut total_bytes_len = 0;
                            for _ in 0..count {

                                #(
                                    total_bytes_len += <#ty_l as Unwiring>::bytes_length(wire, 1)?;
                                )*

                            }
                            Ok(total_bytes_len)
                        }

                    }

                };

                return expaned.into();
            }
            syn::Fields::Unit => {
                let expaned = quote::quote! {
                    impl Unwiring for #ident {
                        const FIXED_SIZE: usize = 1;
                        const MIXED: bool = false;
                        #[inline]
                        fn unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
                            async move {
                                wire.unwiring::<()>().await?;
                                Ok(Self)
                            }
                        }
                        #[inline(always)]
                        fn sync_unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> Result<Self, std::io::Error> where W: std::io::Read {
                            wire.sync_unwiring::<()>()?;
                            Ok(Self)
                        }
                        #[inline(always)]
                        fn bytes_length<W: wiring::prelude::Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
                        where
                            W: std::io::Read,
                        {
                            wire.advance_position(1 * count)?;
                            Ok(1 * count)
                        }
                    }

                };

                return expaned.into();
            }
        },
        Data::Enum(enum_data) => {
            let variants = enum_data.variants;
            let len = variants.len();
            let tag = get_tag(&attrs, len);

            let cases = variants
                .iter()
                .enumerate()
                .map(|(index, Variant { ident, fields, .. })| match fields {
                    syn::Fields::Named(named) => {
                        for field in named.named.iter() {
                            let field_type = field.ty.clone();
                            let tokens = quote! {
                                + <#field_type as wiring::prelude::Unwiring>::FIXED_SIZE
                            };

                            fixed_size_tokens.extend(tokens);
                        }

                        let n_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                             #index => {
                                Self::#ident {
                                    #(#n_field: wire.unwiring().await?,)*
                                }
                            },
                        }
                    }
                    syn::Fields::Unnamed(unamed) => {
                        for field in unamed.unnamed.iter() {
                            let field_type = field.ty.clone();
                            let tokens = quote! {
                                + <#field_type as wiring::prelude::Unwiring>::FIXED_SIZE
                            };

                            fixed_size_tokens.extend(tokens);
                        }

                        let n_ty = unamed.unnamed.iter().map(|n| &n.ty);
                        quote::quote! {
                            #index => {
                                Self::#ident (
                                    #(wire.unwiring::<#n_ty>().await?,)*
                                )
                            },
                        }
                    }
                    syn::Fields::Unit => {
                        quote::quote! {
                            #index => {
                                Self::#ident
                            },
                        }
                    }
                });

            let cases_s = variants
                .iter()
                .enumerate()
                .map(|(index, Variant { ident, fields, .. })| match fields {
                    syn::Fields::Named(named) => {
                        let n_field = named.named.iter().map(|n| &n.ident);
                        quote::quote! {
                             #index => {
                                Self::#ident {
                                    #(#n_field: wire.sync_unwiring()?,)*
                                }
                            },
                        }
                    }
                    syn::Fields::Unnamed(unamed) => {
                        let n_ty = unamed.unnamed.iter().map(|n| &n.ty);
                        quote::quote! {
                            #index => {
                                Self::#ident (
                                    #(wire.sync_unwiring::<#n_ty>()?,)*
                                )
                            },
                        }
                    }
                    syn::Fields::Unit => {
                        quote::quote! {
                            #index => {
                                Self::#ident
                            },
                        }
                    }
                });

            let cases_l = variants
                .iter()
                .enumerate()
                .map(|(index, Variant { fields, .. })| match fields {
                    syn::Fields::Named(named) => {
                        let n_ty = named.named.iter().map(|n| &n.ty);
                        quote::quote! {
                             #index => {
                                #(
                                    total_bytes_len += <#n_ty as Unwiring>::bytes_length(wire, 1)?;
                                )*
                            },
                        }
                    }
                    syn::Fields::Unnamed(unamed) => {
                        let n_ty = unamed.unnamed.iter().map(|n| &n.ty);
                        quote::quote! {
                            #index => {
                                #(
                                    total_bytes_len += <#n_ty as Unwiring>::bytes_length(wire, 1)?;
                                )*
                            },
                        }
                    }
                    syn::Fields::Unit => {
                        quote::quote! {
                            #index => {
                                // no to advance for units as their tagged variant already included in the length.
                            },
                        }
                    }
                });
            let expanded = quote::quote! {

                impl #impl_g Unwiring for #ident #ty_g #wh_g {
                    #[inline]
                    fn unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
                        async move {
                            let taged = wire.unwiring::<#tag>().await? as usize;
                            let r = match taged {
                                #(#cases)*
                                _e => {
                                    let msg = format!("Unexpected variant for {}", stringify!(#ident));
                                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,msg))
                                }
                            };
                            Ok(r)
                        }
                    }
                    #[inline]
                    fn sync_unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> Result<Self, std::io::Error> where W: std::io::Read {
                        let taged = #tag::sync_unwiring(wire)? as usize;
                        let r = match taged {
                            #(#cases_s)*
                            _e => {
                                let msg = format!("Unexpected variant for {}", stringify!(#ident));
                                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,msg))
                            }
                        };
                        Ok(r)
                    }
                    #[inline(always)]
                    fn bytes_length<W: wiring::prelude::Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
                    where
                        W: std::io::Read,
                    {

                        let mut total_bytes_len = std::mem::size_of::<#tag>() as u64 * count;
                        for _ in 0..count {
                            let taged = #tag::sync_unwiring(wire)? as usize;
                            match taged {
                                #(#cases_l)*
                                _e => {
                                    let msg = format!("Unexpected variant for {}", stringify!(#ident));
                                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,msg))
                                }
                            }
                        }
                        Ok(total_bytes_len)

                    }

                }

            };
            return expanded.into();
        }
        _ => {
            panic!("Unwiring doesn't support union")
        }
    }
}

#[proc_macro_attribute]
pub fn fixed(_attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    item
}
