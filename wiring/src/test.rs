use crate as wiring;

use wiring::prelude::*;

#[derive(Unwiring, Wiring, PartialEq, Eq)]
struct Named<T> {
    #[concat_start]
    point: Point,
    #[concat_end]
    field2: Point,
    generic: T,
}

#[derive(Unwiring, Wiring, PartialEq, Eq)]
struct Point {
    #[concat_start]
    x: u32,
    #[concat_end]
    y: u32,
}

#[derive(Unwiring, Wiring, PartialEq, Eq)]
struct Unamed<T>(T, u128);

#[test]
fn unamed_test() {
    let mut wire = Vec::new();
    let data = Unamed("Hello".to_string(), 128);
    (&mut wire).sync_wire(&data).unwrap();
    let b = &wire[..];
    let mut unwire = BufUnWire::new(b);

    let d = unwire.unwire::<Unamed<String>>().unwrap();
    assert!(data == d);
}
