#[proc_macro_derive(Wiring, attributes(tag))]
pub fn wiring_proc_macro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    use proc_macro2::Span;
    use quote::quote;
    use syn::{
        parse_macro_input, parse_quote, Data, DeriveInput, Fields, GenericParam, Index, Lifetime, Variant, WhereClause,
        WherePredicate,
    };

    let input = parse_macro_input!(input as DeriveInput);
    let this = input.ident;
    let data = input.data;
    let g = input.generics.clone();
    let attrs = input.attrs;
    let (impl_g, ty_g, wh_g) = g.split_for_impl();

    let mut ref_g = input.generics.clone();

    let l = Lifetime::new("'wiring", Span::call_site());
    let life_time = syn::LifetimeParam::new(l);
    let life_time = GenericParam::Lifetime(life_time);
    ref_g.params.push(life_time);

    match data {
        Data::Struct(data) => match data.fields {
            syn::Fields::Named(fields) => {
                let named = fields.named;
                let name = named.iter().map(|f| &f.ident);

                let r_name = named.iter().map(|f| &f.ident);

                match &mut ref_g.where_clause {
                    None => {
                        let n = named.clone();
                        let mut it = n.iter();
                        let mut new: WhereClause = parse_quote!(where);
                        while let Some(n) = it.next() {
                            let ty = &n.ty;
                            let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);

                            new.predicates.push(parsed);
                        }
                        ref_g.where_clause = Some(new);
                    }
                    Some(where_c) => {
                        let n = named.clone();
                        let mut it = n.iter();
                        while let Some(n) = it.next() {
                            let ty = &n.ty;
                            let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);
                            where_c.predicates.push(parsed);
                        }
                    }
                }

                let (r_impl_g, _, r_wh_g) = ref_g.split_for_impl();

                let expaned = quote! {

                    impl #impl_g Wiring for #this #ty_g #wh_g {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(self.#name.wiring(wire).await?;)*
                                Ok(())
                            }
                        }
                    }

                    impl #r_impl_g Wiring for &'wiring #this #ty_g #r_wh_g {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #((&self.#r_name).wiring(wire).await?;)*
                                Ok(())
                            }
                        }
                    }
                };

                return expaned.into();
            }
            syn::Fields::Unnamed(fields) => {
                let unamed = fields.unnamed;
                let index = unamed.iter().enumerate().map(|(index, _)| Index::from(index));
                let r_index = unamed.iter().enumerate().map(|(index, _)| Index::from(index));

                match &mut ref_g.where_clause {
                    None => {
                        let n = unamed.clone();
                        let mut it = n.iter();
                        let mut new: WhereClause = parse_quote!(where);
                        while let Some(n) = it.next() {
                            let ty = &n.ty;
                            let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);

                            new.predicates.push(parsed);
                        }
                        ref_g.where_clause = Some(new);
                    }
                    Some(where_c) => {
                        let n = unamed.clone();
                        let mut it = n.iter();
                        while let Some(n) = it.next() {
                            let ty = &n.ty;
                            let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);
                            where_c.predicates.push(parsed);
                        }
                    }
                }

                let (r_impl_g, _, r_wh_g) = ref_g.split_for_impl();

                let expaned = quote::quote! {

                    impl #impl_g Wiring for #this #ty_g #wh_g {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #(self.#index.wiring(wire).await?;)*
                                Ok(())
                            }
                        }
                    }

                    impl #r_impl_g Wiring for &'wiring #this #ty_g #r_wh_g {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                #((&self.#r_index).wiring(wire).await?;)*
                                Ok(())
                            }
                        }
                    }
                };

                return expaned.into();
            }
            syn::Fields::Unit => {
                let expanded = quote::quote! {
                    impl Wiring for #this {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                ().wiring(wire).await
                            }
                        }
                    }
                    impl<'wiring> wiring::prelude::Wiring for &'wiring #this {
                        #[inline]
                        fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                            async move {
                                ().wiring(wire).await
                            }
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
            let safe_2 = safe.into_iter().flatten();

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

            let cases_ref = variants
                .iter()
                .enumerate()
                .map(|(idx, Variant { ident, fields, .. })| match fields {
                    Fields::Named(named) => {
                        let n_field = named.named.iter().map(|n| &n.ident);
                        let named = &named.named;
                        let f_match = named.iter().map(|n| {
                            let f_match = &n.ident;
                            let field = named.iter().map(|n| {
                                let f_name = &n.ident;
                                quote! {
                                    #f_name,
                                }
                            });
                            quote! {
                                #[allow(unused_variables)]
                                if let #this::#ident {#(#field )*} = self {
                                    #f_match.wiring(wire).await?;
                                };
                            }
                        });

                        quote::quote! {
                            #this::#ident { #(#n_field, )* } => {

                                (#idx as #tag).wiring(wire).await?;

                                #(
                                   #f_match
                                )*

                            }
                        }
                    }

                    Fields::Unnamed(unamed) => {
                        let unamed = &unamed.unnamed;
                        let nn_field = unamed.iter().enumerate().map(|(i, _)| {
                            let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            f_idx
                        });

                        let f_match = unamed.iter().enumerate().map(|(i, _)| {
                            let f_match = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                            let field = unamed.iter().enumerate().map(|(i, _)| {
                                let f_idx = syn::Ident::new(&format!("field{}", i), proc_macro2::Span::call_site());
                                quote! {
                                    #f_idx,
                                }
                            });
                            quote! {
                                #[allow(unused_variables)]
                                if let #this::#ident (#(#field )*) = self {
                                    #f_match.wiring(wire).await?;
                                };
                            }
                        });

                        quote::quote! {
                            #this::#ident ( #(#nn_field, )* ) => {

                                (#idx as #tag).wiring(wire).await?;

                                #(
                                   #f_match
                                )*

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

            for Variant { fields, .. } in variants.iter() {
                match fields {
                    Fields::Named(named) => match &mut ref_g.where_clause {
                        None => {
                            let n = named.named.clone();
                            let mut it = n.iter();
                            let mut new: WhereClause = parse_quote!(where);
                            while let Some(n) = it.next() {
                                let ty = &n.ty;
                                let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);
                                new.predicates.push(parsed);
                            }
                            ref_g.where_clause = Some(new);
                        }
                        Some(where_c) => {
                            let n = named.named.clone();
                            let mut it = n.iter();
                            while let Some(n) = it.next() {
                                let ty = &n.ty;
                                let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);
                                where_c.predicates.push(parsed);
                            }
                        }
                    },
                    Fields::Unnamed(unamed) => match &mut ref_g.where_clause {
                        None => {
                            let n = unamed.unnamed.clone();
                            let mut it = n.iter();
                            let mut new: WhereClause = parse_quote!(where);
                            while let Some(n) = it.next() {
                                let ty = &n.ty;
                                let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);

                                new.predicates.push(parsed);
                            }
                            ref_g.where_clause = Some(new);
                        }
                        Some(where_c) => {
                            let n = unamed.unnamed.clone();
                            let mut it = n.iter();
                            while let Some(n) = it.next() {
                                let ty = &n.ty;
                                let parsed: WherePredicate = parse_quote!(&'wiring #ty: Wiring);
                                where_c.predicates.push(parsed);
                            }
                        }
                    },
                    _ => (),
                }
            }

            let (r_impl_g, _, r_wh_g) = ref_g.split_for_impl();

            let expanded = quote::quote! {

                impl #impl_g Wiring for #this #ty_g #wh_g {
                    const SAFE: bool = true #( && #safe_1)*;
                    #[inline]
                    fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                        async move {
                            match self {
                                #(#cases)*
                            }
                            Ok(())
                        }
                    }
                }

                impl #r_impl_g Wiring for &'wiring #this #ty_g #r_wh_g {
                    const SAFE: bool = true #( && #safe_2)*;
                    #[inline]
                    #[allow(unused_variables)]
                    fn wiring<W: wiring::prelude::Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
                        async move {
                            match self {
                                #(#cases_ref)*
                            }
                            Ok(())
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
    use syn::{parse_macro_input, Data, DeriveInput, Variant};
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let data = input.data;
    let g = input.generics.clone();
    let attrs = input.attrs;
    let (impl_g, ty_g, wh_g) = g.split_for_impl();
    match data {
        Data::Struct(data) => match data.fields {
            syn::Fields::Named(fields) => {
                let named = fields.named;
                let name = named.iter().map(|f| &f.ident);

                let expaned = quote::quote! {

                    impl #impl_g Unwiring for #ident #ty_g #wh_g {
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
                    }

                };

                return expaned.into();
            }
            syn::Fields::Unnamed(fields) => {
                let unamed = fields.unnamed;
                let ty = unamed.iter().map(|i| &i.ty);

                let expaned = quote::quote! {

                    impl #impl_g Unwiring for #ident #ty_g #wh_g {
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
                    }

                };

                return expaned.into();
            }
            syn::Fields::Unit => {
                let expaned = quote::quote! {
                    impl Unwiring for #ident {
                        #[inline]
                        fn unwiring<W: wiring::prelude::Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
                            async move {
                                wire.unwiring::<()>().await?;
                                Ok(Self)
                            }
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
                }

            };
            return expanded.into();
        }
        _ => {
            panic!("Unwiring doesn't support union")
        }
    }
}
