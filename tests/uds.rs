#![cfg(any(unix, macos))]
#![feature(async_await, await_macro, futures_api)]
use std::io::{Read, Write};
use std::os::unix::net::UnixStream as StdStream;
use std::thread;

use futures::executor;
use futures::future::FutureObj;
use futures::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use futures::Stream;
use futures::StreamExt;
use log::{error, info};
use tempdir::TempDir;

use romio::uds::{UnixListener, UnixStream};

type Error = Box<dyn std::error::Error + 'static>;

const THE_WINTERS_TALE: &[u8] = b"
                    Each your doing,
    So singular in each particular,
    Crowns what you are doing in the present deed,
    That all your acts are queens.
";

#[test]
fn listener_reads() -> Result<(), Error> {
    drop(env_logger::try_init());
    let tmp_dir = TempDir::new("listener_reads")?;
    let file_path = tmp_dir.path().join("sock");

    let listener = UnixListener::bind(&file_path)?;
    let file_path = listener.local_addr()?;

    // client thread
    thread::spawn(move || {
        let file_path = file_path.as_pathname().unwrap();
        let mut client = StdStream::connect(&file_path).unwrap();
        client.write_all(THE_WINTERS_TALE).unwrap();
    });

    executor::block_on(async {
        let mut buf = vec![0; THE_WINTERS_TALE.len()];
        let mut incoming = listener.incoming();
        let mut stream = await!(incoming.next()).unwrap().unwrap();
        await!(stream.read_exact(&mut buf)).unwrap();
        assert_eq!(buf, THE_WINTERS_TALE);
    });

    Ok(())
}

#[test]
fn listener_writes() -> Result<(), Error> {
    drop(env_logger::try_init());
    let tmp_dir = TempDir::new("listener_writes")?;
    let file_path = tmp_dir.path().join("sock");

    let listener = UnixListener::bind(&file_path)?;
    let file_path = listener.local_addr()?;

    // client thread
    thread::spawn(move || {
        let mut buf = vec![0; THE_WINTERS_TALE.len()];
        let file_path = file_path.as_pathname().unwrap();
        let mut client = StdStream::connect(&file_path).unwrap();
        client.read_exact(&mut buf).unwrap();
        assert_eq!(buf, THE_WINTERS_TALE);
    });

    executor::block_on(async {
        let mut incoming = listener.incoming();
        let mut stream = await!(incoming.next()).unwrap().unwrap();
        await!(stream.write_all(THE_WINTERS_TALE)).unwrap();
    });

    Ok(())
}

#[test]
fn both_sides_async_using_threadpool() -> Result<(), Error>{
    drop(env_logger::try_init());
    let tmp_dir = TempDir::new("both_sides_async")?;
    let file_path = tmp_dir.path().join("sock");

    let listener = UnixListener::bind(&file_path)?;
    let file_path = listener.local_addr()?;

    let mut pool = executor::ThreadPool::new().unwrap();

    pool.run(FutureObj::from(Box::pin(async move {
        let file_path = file_path.as_pathname().unwrap();
        let mut client = await!(UnixStream::connect(&file_path)).unwrap();
        await!(client.write_all(THE_WINTERS_TALE)).unwrap();
    })));

    pool.run(FutureObj::from(Box::pin(async {
        let mut buf = vec![0; THE_WINTERS_TALE.len()];
        let mut incoming = listener.incoming();
        let mut stream = await!(incoming.next()).unwrap().unwrap();
        await!(stream.read_exact(&mut buf)).unwrap();
        assert_eq!(buf, THE_WINTERS_TALE);
    })));

    Ok(())
}

#[test]
fn pair() -> Result<(), Error> {
    drop(env_logger::try_init());

    let (mut server, mut client) = UnixStream::pair().expect("Could not build pair");

    // client thread
    let fut_bytes_test = async {
        let mut buf = vec![0; THE_WINTERS_TALE.len()];
        await!(client.read_exact(&mut buf)).unwrap();
        assert_eq!(buf, THE_WINTERS_TALE);
    };

    executor::block_on(async {
        await!(server.write_all(THE_WINTERS_TALE)).unwrap();
    });

    executor::block_on(fut_bytes_test);

    Ok(())
}

pub struct RomioReader {
    inner: UnixStream,
    buffer: bytes::BytesMut
}

impl Unpin for RomioReader {}

impl RomioReader {
    pub fn new(inner: UnixStream) -> RomioReader {
        let mut buff = bytes::BytesMut::with_capacity(10_000_000);
        buff.resize(buff.capacity(), 0u8);
        RomioReader {
            inner: inner,
            buffer: buff
        }
    }
}

impl Stream for RomioReader {
    type Item = Vec<u8>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, lw: &std::task::LocalWaker) -> futures::Poll<Option<Self::Item>> {
        let this = &mut *self;
        match this.inner.poll_read(lw, this.buffer.as_mut()) {
            futures::Poll::Pending => futures::Poll::Pending,
            futures::Poll::Ready(Err(e)) => {
                error!("Failed to poll_read {:?}", e);
                futures::Poll::Ready(None)
            },
            futures::Poll::Ready(Ok(bytes_read)) => {
                info!("Read {} bytes", bytes_read);
                if bytes_read == 0 {
                    return futures::Poll::Ready(None);
                }

                let (r, _) = this.buffer.split_at(bytes_read);
                futures::Poll::Ready(Some(r.to_vec()))
            }
        }
    }
}

impl From<UnixStream> for RomioReader {
    fn from(v: UnixStream) -> Self {
        RomioReader::new(v)
    }
}

#[test]
fn reads_bytes() {
    drop(env_logger::try_init());

    let (mut server, client) = UnixStream::pair().expect("Could not build pair");

    let send_complete = std::thread::spawn(move || {
        let bytes = "{\"key1\":\"key with a paren set {}\",\"key2\":12345}{\"another\":\"part being sent\"}".as_bytes();
        let f = server.write_all(bytes);

        executor::block_on(f).expect("Failed to send");

        executor::block_on(server.close()).expect("Failed to close");
    });

    let fut_string = async {
        let reader: RomioReader = client.into();
        let bytes: Vec<u8> = await!(reader
            .fold(vec![], |mut agg, b| {
                agg.extend(b);
                futures::future::ready(agg)
            })
        );
        String::from_utf8(bytes).expect("Failed to convert to string")
    };

    send_complete.join().expect("Failed to send");

    let string = futures::executor::block_on(fut_string);

    assert_eq!(string, "{\"key1\":\"key with a paren set {}\",\"key2\":12345}{\"another\":\"part being sent\"}".to_string());
}
