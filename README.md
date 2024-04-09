
# Wiring

Welcome to Wiring, an advanced asynchronous serialization framework for Rust. Wiring is designed to enable efficient communication patterns in concurrent applications by allowing the transmission of types and channels over asynchronous streams.

## Key Features

- **Asynchronous Serialization**: Serialize and deserialize data structures asynchronously with minimal overhead.
- **Channel Support**: Seamlessly send and receive Rust channels over the network, fully leveraging Rust's powerful concurrency model.
- **Stream Compatibility**: Works with any async stream, including TcpStream, for flexible integration with different transport layers.
- **Nested Channels**: Support for nested channels, enabling complex communication structures such as channel-over-channel communication.
- **WIP: WebAssembly (WASM) Support**: Run your Rust code in the browser with full WASM support, bringing high performance to web applications.
- **Procedural Macro**: Use the `Wiring`, `Unwiring` macros to automatically derive de/serialization, with an optional custom numeric type tag for larger enums.

## Procedural Macro Usage

Wiring provides a procedural macros `Wiring, Unwiring` to facilitate the automatic implemenations, tagging of enum variants. The macro can be used as follows:

```rust!
#[derive(Wiring, Unwiring)]
#[tag(u8)]
enum MyEnum {
    VariantOne,
    VariantN,
}
```

The `tag` attribute is optional. By default, a `u16` is used for tagging, which ensures future-proofing. However, for better space and bandwidth efficiency, it is recommended to use a `u8`.

### Notes

- **Tag Type Range**: The default choice of `u16` allows support for up to `u16::MAX` variants. If your enum has fewer or more variants, consider specifying a custom tag type using the tag attribute.
- **Future Expansion**: While `u8` is recommended for space and bandwidth efficiency, it’s prudent to specify a larger tag type upfront to prevent breaking changes in the future.
- **Interoperability**: Ensure that the chosen tag type aligns with the expectations of any systems that interact with the serialized data to maintain compatibility.

## Getting Started

To include Wiring in your Rust project, add the following to your `Cargo.toml` file:

```toml
[dependencies]
wiring = "0.1"
```
Check [examples](examples) folder.


## Contributing

Contributions to Wiring are welcome! Please read our contributing guidelines to get started on helping improve the project.

## License

Wiring is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- A big thank you to all contributors and users of Wiring!