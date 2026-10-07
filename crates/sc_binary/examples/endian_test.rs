use sc_binary::ByteReader;
fn main() {
    let mut r = ByteReader::from(&[0x0d, 0x00][..]);
    println!("read_u16(0d 00) = {}", r.read_u16().unwrap());
    let mut r2 = ByteReader::from(&[0x00, 0x0d][..]);
    println!("read_u16(00 0d) = {}", r2.read_u16().unwrap());
}
