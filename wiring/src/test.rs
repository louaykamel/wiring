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

#[derive(Unwiring, Wiring, PartialEq, Eq, Debug)]
struct Unamed<T>(T, u128);

#[test]
fn unamed_test() {
    let mut wire = Vec::new();
    let data = Unamed("Hello".to_string(), 128);
    (&mut wire).sync_wire(&data).unwrap();
    let b = &wire[..];
    let mut unwire = BufUnWire::new(b);

    let d = unwire.unwire::<Unamed<String>>().unwrap();
    assert_eq!(data, d);
}

#[derive(Wiring, Unwiring)]
pub struct Abilities {
    #[fixed]
    pub walk_speed: f32,
    pub fly_speed: f32,
}

#[derive(Debug, PartialEq, Unwiring, Wiring)]
struct Syncopated {
    small: u8,
    big: u64,
}
#[derive(Debug, PartialEq, Unwiring, Wiring)]
struct Several {
    vec: Vec<Syncopated>,
}

#[test]
fn unaligned_read() {
    let a = Syncopated { small: 1, big: 2 };
    let mut buf = Vec::new();
    BufWire::new(&mut buf).wire(&a).unwrap();
    let b = BufUnWire::new(buf.as_slice()).unwire::<Syncopated>().unwrap();

    assert_eq!(a, b);
}

#[test]
fn incorrect_size_estimation() {
    let a = Several {
        vec: vec![Syncopated { small: 1, big: 2 }, Syncopated { small: 3, big: 4 }],
    };
    let mut buf = Vec::new();
    BufWire::new(&mut buf).wire(&a).unwrap();
    let b = BufUnWire::new(buf.as_slice()).unwire::<Several>().unwrap();

    assert_eq!(a, b);
}
