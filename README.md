
Used to convert any stream into a put request through writer and upload it to the server, so as to achieve file-free landing.

```rust
fn main() -> std::io::Result<()> {

    let writer = http_writer::HttpWriter::new("http://192.168.100.100/archive.tar.xz").unwrap();

    //
    // cargo add xz2 tar
    //
    let encoder = xz2::write::XzEncoder::new(writer, 6);
    let mut tar_builder = tar::Builder::new(encoder);
    tar.append_dir_all("backup/logs", "/var/log")?;

    Ok(())
}
```
