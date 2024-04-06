
# Wiring

Welcome to Wiring, an advanced asynchronous serialization framework for Rust. Wiring is designed to enable efficient communication patterns in concurrent applications by allowing the transmission of types and channels over asynchronous streams.

## Key Features

- **Asynchronous Serialization**: Serialize and deserialize data structures asynchronously with minimal overhead.
- **Channel Support**: Seamlessly send and receive Rust channels over the network, fully leveraging Rust's powerful concurrency model.
- **Stream Compatibility**: Works with any async stream, including TcpStream, for flexible integration with different transport layers.
- **Nested Channels**: Support for nested channels, enabling complex communication structures such as channel-over-channel communication.
- **WIP: WebAssembly (WASM) Support**: Run your Rust code in the browser with full WASM support, bringing high performance to web applications.
- **WIP: Procedural Macro**: Derive Wiring/Unwiring impl.

## Getting Started

To include Wiring in your Rust project, add the following to your `Cargo.toml` file:

```toml
[dependencies]
wiring = "0.1"
```
Check [examples](examples) folder in our github repo.

## Contributing

Contributions to Wiring are welcome! Please read our contributing guidelines to get started on helping improve the project.

## License

Wiring is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- A big thank you to all contributors and users of Wiring!