#[macro_use]
extern crate bencher;

use bencher::{black_box, Bencher};
use rand::random;

use wiring::prelude::*;

const BUF_LEN: usize = 50_000_00;

fn serializing_string(bench: &mut Bencher) {
    let mut wire: Vec<u8> = Vec::new();
    wire.reserve(BUF_LEN);
    let text = format!("hello world");
    bench.iter(|| {
        black_box(&mut wire).sync_wire(&text).unwrap();
        unsafe { wire.set_len(0) }
    });
}

fn serializing_vec_string_100_000(bench: &mut Bencher) {
    let mut wire: Vec<u8> = Vec::new();

    let mut strings = Vec::new();
    for _ in 0..100_000 {
        strings.push(generate_random_string(random::<u8>()))
    }
    wire.reserve(BUF_LEN);

    bench.iter(|| {
        black_box(&mut wire).sync_wire(&strings).unwrap();
        unsafe { wire.set_len(0) }
    });
}

fn serializing_vec_u128_100_000(bench: &mut Bencher) {
    let mut wire: Vec<u8> = Vec::new();

    let mut strings = Vec::new();
    for _ in 0..100_000 {
        strings.push(random::<u128>())
    }
    wire.reserve(BUF_LEN);

    bench.iter(|| {
        black_box(&mut wire).sync_wire(&strings).unwrap();
        unsafe { wire.set_len(0) }
    });
}

// TODO ADD MORE BENCHES FOR MOST TYPES.
benchmark_group!(
    benches,
    serializing_string,
    serializing_vec_string_100_000,
    serializing_vec_u128_100_000
);
benchmark_main!(benches);

fn generate_random_string(length: u8) -> String {
    use rand::{distributions::Alphanumeric, Rng};
    let rng = rand::thread_rng();
    let random_string: String = rng
        .sample_iter(&Alphanumeric)
        .take(length as usize)
        .map(char::from)
        .collect();
    random_string
}
