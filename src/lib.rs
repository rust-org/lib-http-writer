#![feature(let_chains)]
struct ProgressReader<R: std::io::Read> {
    pub inner: R,
    pub pb: std::sync::Weak<indicatif::ProgressBar>,
}

impl<R: std::io::Read> ProgressReader<R> {
    fn new(inner: R, pb: std::sync::Weak<indicatif::ProgressBar>) -> Self {
        ProgressReader { inner, pb }
    }
}

impl<R: std::io::Read> std::io::Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        if bytes_read > 0 {
            self.pb.upgrade().unwrap().inc(bytes_read as u64);
        }
        Ok(bytes_read)
    }
}
//----------------------------------------------------------
pub struct ChannelReader {
    pub inner: Option<std::sync::mpsc::Receiver<Vec<u8>>>,
    pub buffer: Vec<u8>,
    pub pos: usize,
}

impl ChannelReader {
    pub fn new(receiver: std::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        ChannelReader {
            inner: Some(receiver),
            buffer: Vec::new(),
            pos: 0,
        }
    }
    pub fn close(&mut self) {
        if let Some(rx) = self.inner.take() { drop(rx); }
        }
}

impl std::io::Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(receiver) = &self.inner {
            if self.pos >= self.buffer.len() {
                match receiver.recv() {
                    Ok(data) => {
                        self.buffer = data;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
    
            let remaining = self.buffer.len() - self.pos;
            let to_read = remaining.min(buf.len());
            buf[..to_read].copy_from_slice(&self.buffer[self.pos..self.pos + to_read]);
            self.pos += to_read;
    
            Ok(to_read)
            }
        else{
            Ok(0)
            }
    }
}

pub struct ChannelWriter {
    pub inner: Option<std::sync::mpsc::Sender<Vec<u8>>>,
}

impl ChannelWriter {
    pub fn new(sender: std::sync::mpsc::Sender<Vec<u8>>) -> Self {
        ChannelWriter { inner:Some(sender) }
    }
    pub fn close(&mut self) {
        if let Some(tx) = self.inner.take() { drop(tx); }
    }
}

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(sender) = &self.inner {
            sender.send(buf.to_vec()).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, format!("ChannelWriter: Failed to send data {}", e))
                })?;
            Ok(buf.len())
            }
        else {
            Ok(0)
            }
        }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
//----------------------------------------------------------
fn reqwest_error_to_io_error(error: reqwest::Error) -> std::io::Error {
    if error.is_timeout() {
        std::io::Error::new(std::io::ErrorKind::TimedOut, format!("Request timed out: {}", error))
    } else if error.is_request() {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Request error: {}", error))
    } else if error.is_redirect() {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Redirect error: {}", error))
    } else if error.is_status() {
        std::io::Error::new(std::io::ErrorKind::Other, format!("HTTP status error: {}", error))
    } else if error.is_body() {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Body error: {}", error))
    } else if error.is_decode() {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Decode error: {}", error))
    } else if error.is_connect() {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Connect error: {}", error))
    } else if error.is_builder() {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Builder error: {}", error))
    } else {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Unknown error: {}", error))
    }
}

pub struct HttpWriter {
    pub tx: ChannelWriter,
    pub pos: u64,
    pub uploader_handle: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

impl HttpWriter {
    pub fn new_with_config(url: &str, client: &reqwest::blocking::Client, prog: indicatif::ProgressBar) -> std::io::Result<Self> {
        put_zero_file(&client, &url)?;
        let (tx, rx) = std::sync::mpsc::channel();

        let reader = ChannelReader::new(rx);
        let url = url.to_string();
        let client = client.clone();
        let uploader_handle: std::thread::JoinHandle<Result<(), std::io::Error>> = std::thread::spawn(move || {
            let pb = std::sync::Arc::new(prog);
            let response = client
                .put(url)
                .header("Transfer-Encoding", "chunked")
                .body(reqwest::blocking::Body::new(ProgressReader::new(reader, std::sync::Arc::downgrade(&pb))))
                .send()
                .map_err(|e|reqwest_error_to_io_error(e))?;

            if !response.status().is_success() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Upload failed, status: {}", response.status()),
                    ));
                }

            pb.finish_with_message("[OK]");
            Ok(())
            });

        Ok(Self {
            tx: ChannelWriter::new(tx),
            pos: 0,
            uploader_handle: Some(uploader_handle),
            })
        }

    pub fn new(url: &str) -> std::io::Result<Self> {
        let client: reqwest::blocking::Client = reqwest::blocking::Client::builder()
            .timeout(None)
            .connect_timeout(std::time::Duration::from_secs(9))
            // .proxy(reqwest::Proxy::http("http://192.168.0.102:8888").unwrap())
            .build()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let pb = indicatif::ProgressBar::new_spinner();
        pb.set_style(
            indicatif::ProgressStyle::default_spinner()
            .template("{spinner:.green} {bytes} bytes uploaded at {bytes_per_sec} {msg}")
            .unwrap()
            //.tick_chars("/|\\- "),
            );
        Self::new_with_config(url, &client, pb)
        }
}

impl std::io::Write for HttpWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let bytes_written = buf.len();
        // self.tx.write(buf)?;
        if let Err(e) = self.tx.write(buf) && let Some(uploader) = self.uploader_handle.take() {
            if uploader.is_finished(){
                if let Err(e)  = uploader.join().unwrap(){
                    return Err(e);
                    }
                }
            else{
                self.uploader_handle = Some(uploader);
                }
            return Err(e);
            }
        self.pos += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for HttpWriter {
    fn drop(&mut self) {
        self.tx.close();
        if let Some(handle) = self.uploader_handle.take() {
            handle.join().expect("Upload thread cannot be terminated").unwrap();
        }
    }
}

pub fn put_zero_file(client: &reqwest::blocking::Client, url: &str) -> Result<(), std::io::Error> {
    let res = client
        .request(reqwest::Method::PUT, url)
        .timeout(std::time::Duration::from_secs(9))
        .body("")
        .send()
        .map_err(|e|reqwest_error_to_io_error(e))?;

    if !res.status().is_success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported, 
            format!("PUT request failed with status: {}", res.status())
        ));
    }

    Ok(())
}