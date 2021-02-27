### Intro

Recently I experimented with writing a simple Rust program that touches a few important system programming concepts: io, server-client architecture and concurrency. The goals were: 1) to write code that should be idiomatic to Rust, 2) to write scalable, non-blocking solution and 3) to write quality code (no warnings, dealing with errors and edge cases).
The desired functionality is to synchronize a file, that is being continuously appended to, between multiple machines. 
Below is the short description of the process and of the Rust ecosystem bits that I used.

Github: https://github.com/jakubtyrcha/filerep

### Async and tokio

Rust has async/await model with allows to express functions as coroutines. After adding `async` keyword to a function declaration it becomes a coroutine and it's return type is generic `Future`. Coroutines are compiled to a state machine, where each `await` call introduces a new state. This allows to suspend execution, return the flow control to the scheduler and resume from the same location later on.

Std lib support for async code is a bit bare-bones. Here's where custom solutions come to the rescue. One of them is Tokio, which has its async aware primitives, interfaces, a family of schedulers and other tools.

Tokio comes with it's own interfaces that support Async variant of typical blocking traits for things like file or network io, for example `AsyncRead` and `AsyncWrite`. For ease of use traits `AsyncReadExt` and `AsyncWriteExt` exist, which simplify dealing with potential failures and retries.

Concurrency is achieved via spawning a new task. 
```rust
tokio::spawn(async {
    sleep(Duration::from_millis(100)).await;
    println!("Done!");  
})
```

**Caution!** Care meeds to be taken when using synchronization primitives shared between sync and async code. Tokio uses its own scheduler-aware primitives. Using regular primitives might result in a deadlock.
As an example, the `Notify` crate uses its own thread that sends file change events using a standard mpsc queue. To avoid a deadlock, we need to use non-blocking functions (like `try_recv`) and return control flow to the scheduler using `yield_now` from the tokio lib.
```rust
async move {
    loop {
        let mut write_detected = false;
        let mut drained = false;
        while !drained && !write_detected {
            match file_rx.try_recv() { // !!!!!!!!!!!!!!
                Ok(event) => {
                    match event {
                        notify::DebouncedEvent::Write(_) => {
                            write_detected = true;
                        }
                        _ => {}
                    }
                },
                Err(TryRecvError::Empty) => { drained = true; },
                Err(TryRecvError::Disconnected) => {
                    return Err(TryRecvError::Disconnected);
                }
            }
        }
        ...

        tokio::task::yield_now().await; // !!!!!!!!!!!!!!!
    }
}
```

### Sending higher-level binary data

Tokio variant of `TcpSocket` implements `AsyncRead` and `AsyncWrite` traits, which allows the user to read and write arrays of bytes. Using this level of abstraction requires the user to deal with details such as byte ordering, incomplete packets etc. Luckily, Rust comes with `Stream` and `Sink` traits, which operate on data structures. For tokio, there's the tokio_util crate with `Framed` object and `Encoder`|`Decoder` traits. `Framed` packages object implementing async io with `Encoder` or `Decoder` that tells it how to interpret the data.

Encoder trait takes and object and writes slices of bytes into the stream. Take a note that this handshake example always outputs the same data, so it's specialized for the empty tuple.
```rust
pub struct HandshakeEncoder {} // dummy encoder

impl Encoder<()> for HandshakeEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, _: (), dst: &mut BytesMut) -> Result<(), Self::Error> {
        let data = get_handshake_msg();
        dst.extend_from_slice(&data[..]);
        Ok(())
    }
}
```

The decoder looks at the incoming byte stream and returns `Ok(None)` until there's enough data to produce the final object (or `Err(...)` on error). Once it's done, the pointer is advanced and we return the object. As with encoding, for this handshake example we don't care about the object itself, all we need is the information that the right data arrived. Therefore we return a decorated empty tuple.
```rust
impl Decoder for HandshakeDecoder {
    type Item = ();
    type Error = std::io::Error;

    fn decode(
        &mut self,
        src: &mut BytesMut
    ) -> Result<Option<Self::Item>, Self::Error> {
        let expected = get_handshake_msg();

        if src.len() < expected.len() {
            return Ok(None);
        }

        if &src[..expected.len()] != expected {
            return Err(Self::Error::new(std::io::ErrorKind::InvalidData, "handshake failed"));
        }
        
        src.advance(expected.len());
        return Ok(Some(()));
    }
}
```

### Thread-safe data sharing

To share the file changes between concurrent connections I made a simple list structure. 
```rust
struct Node {
    next : Arc<RwLock<Option<Node>>>,
    data : Vec<u8>
}
```
The `data` field is read only, so it doesn't need any special treatment. The `next` field can be read by multiple connection tasks and it can be locked for write by the file reader task. `Arc` is the Rust way to express thread-safe shared pointer. `Arc` combined with a mutex-like structure is the typical pattern for a thread safe shared data.

Producer updating the tail:
```rust
let next_tail;
{
    let mut write_guard = write_tail.write().await;
    let node = write_guard.as_mut().unwrap();
    next_tail = node.append(Node::new(buffer)).await;
}
write_tail = next_tail;
list_tx.send(())?; // signaling that a new node was appended
```

Consumer:
```rust
let mut next_head = None;
{
    let read_guard = read_head.read().await;
    if let Some(node) = read_guard.as_ref() {
        next_head = Some(node.next.clone());
        ... // node can be safely read in this scope
    }
}
if let Some(next_head) = next_head {
    read_head = next_head;
}
else {
    connection_list_rx.changed().await?; // suspend until a new tail is appended
}
```

### Errors

Rust has a few error handling patterns that simplify writing safe code. One of them is `operator ?`, which checks the returned `Result` type and returns on error. An alternative is `expect` call which results in `panic!` (Rust program termination on runtime error) on failure.

```rust
let mut file = File::open(Path::new(&path)).await?;
let mut buffer = Vec::new();
file.read_to_end(&mut buffer).await?;
```

```rust
let listener = TcpListener::bind(&address).await.expect("unable to bind TCP listener"); 
```

**Caution!** For `async` blocks and lambdas, there's no way to specify the return type. This might result in cryptic errors about a failure to infer the returned type. Currently the recommended solution is to specify the returned type explicitely by annotation the last expression in the block:
```rust
async {
    ...
    // This is here because the compiler can't infer the return type
    #[allow(unreachable_code)]
    Ok::<_, Box<dyn Error + Send + Sync>>(())
}
```
For multiple different error types that have to be returned, we need a polymorphic (`dyn`) combination of `Error`, `Send` and `Sync` traits (task return type requirement, so it can be shared between threads by the scheduler).


### Testing

Rust comes with unit and integration test infrastructure built into it's build system. Calling `cargo test` from command line executes all the tests. Tests are created by annotating the test function with a special macro. Tokio provides a special macro to test async functions:
```
#[tokio::test]
async fn test_multiple_connections() {
    ...
}
```

### Package manager

Rust has a convenient centralised package ecosystem: cargo. Some of the crates I used:

`Notify` - File watcher with events debouncing of file changed events. It's based on OS file watch APIs. Internally it seems to use a separate thread to emit events and it doesn't support async code.

`Clap` - Declarative CLI. Example:
```rust
let matches = App::new("filerep")
    .arg(Arg::with_name("server").long("server").takes_value(false))
    .arg(Arg::with_name("port").required(true))
    .arg(Arg::with_name("path").required(true))
    .arg(Arg::with_name("address").required_unless("server"))
    .get_matches();
```