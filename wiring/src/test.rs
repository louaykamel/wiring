use crate as wiring;

use wiring::prelude::*;

#[derive(Unwiring, Wiring, PartialEq, Eq)]
struct Named<T> {
    #[fixed(2)]
    point: Point,
    field2: Point,
    generic: T,
}

#[derive(Unwiring, Wiring, PartialEq, Eq)]
struct Point {
    #[fixed]
    x: u32,
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

#[derive(Wiring, Unwiring)]
pub struct Abilities {
    #[fixed]
    pub walk_speed: f32,
    pub fly_speed: f32,
}
