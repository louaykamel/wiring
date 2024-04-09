use crate::prelude::*;

#[derive(Wiring, Unwiring, PartialEq, Eq)]
struct Named<T: Wiring + Unwiring, TT: Wiring + Unwiring> {
    field_1: T,
    field_2: TT,
}

use crate as wiring;

#[derive(Wiring, Unwiring, PartialEq, Eq)]
struct Unamed<T: Wiring + Unwiring, TT: Wiring + Unwiring>(T, TT);

#[derive(Wiring, Unwiring, PartialEq, Eq)]
struct Unit;

#[derive(Unwiring, Wiring, PartialEq, Eq)]
enum Enum<T: Unwiring + Wiring, TT: Wiring + Unwiring> {
    Named { field_1: T, field_2: TT },
    Unamed(T, TT),
    Unit,
}

#[tokio::test]
async fn test_u8() {
    use super::prelude::*;
    let mut wire: Vec<u8> = Vec::new();
    wire.wire(1u8).await.expect("To wire 1u8");
    assert!(wire == vec![1]);

    let mut unwire = std::io::Cursor::new(wire);
    let number: u8 = unwire.unwire().await.expect("To unwire 1u8");

    assert!(number == 1u8);
}

#[tokio::test]
async fn test_struct_named() {
    use super::prelude::*;
    let mut wire: Vec<u8> = Vec::new();
    let named = Named {
        field_1: "Hello".to_string(),
        field_2: "World".to_string(),
    };
    wire.wire(&named).await.expect("To wire Named struct");

    let mut unwire = std::io::Cursor::new(wire);
    let result: Named<String, String> = unwire.unwire().await.expect("To unwire Named struct");
    assert!(named == result);
}

#[tokio::test]
async fn test_struct_unamed() {
    use super::prelude::*;
    let mut wire: Vec<u8> = Vec::new();
    let unamed = Unamed("Hello".to_string(), "World".to_string());
    wire.wire(&unamed).await.expect("To wire Unamed struct");

    let mut unwire = std::io::Cursor::new(wire);
    let result: Unamed<String, String> = unwire.unwire().await.expect("To unwire Unamed struct");
    assert!(unamed == result);
}

#[tokio::test]
async fn test_enum() {
    use super::prelude::*;
    let mut wire: Vec<u8> = Vec::new();
    let named = Enum::Named {
        field_1: "Hello".to_string(),
        field_2: "World".to_string(),
    };
    wire.wire(&named).await.expect("To wire Named enum");

    let unamed = Enum::Unamed("Hello".to_string(), "World".to_string());
    wire.wire(&unamed).await.expect("To wire Unamed enum");

    let mut unwire = std::io::Cursor::new(wire);

    let result: Enum<String, String> = unwire.unwire().await.expect("To unwire Named enum");
    assert!(named == result);
    let result: Enum<String, String> = unwire.unwire().await.expect("To unwire Unamed enum");
    assert!(unamed == result);
}

#[tokio::test]
async fn test_enum_unit() {
    use super::prelude::*;
    let mut wire: Vec<u8> = Vec::new();

    let unit = Enum::<String, String>::Unit;
    wire.wire(&unit).await.expect("To wire Unit enum");

    let mut unwire = std::io::Cursor::new(wire);

    let result: Enum<String, String> = unwire.unwire().await.expect("To unwire Unit enum");
    assert!(Enum::Unit == result);
}
