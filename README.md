# Surf-vcr - Record and Replay HTTP sessions

Surf-vcr is a testing middleware for the [Surf](https://github.com/http-rs/Surf)
HTTP client that records your HTTP sessions to provide deterministic testing of
your HTTP clients.

The high-level design is based on [VCR](https://github.com/vcr/vcr) for Ruby.

Source code is available on [sr.ht](https://git.sr.ht/~rjframe/surf-vcr) and
[Github](https://github.com/rjframe/surf-vcr). Patches may be sent via either
service, but the CI is running on sr.ht.


## Table of Contents

* [Introduction](#introduction)
    * [Install](#application-installation)
    * [Record](#record)
    * [Playback](#playback)
* [License](#license)
* [Contributing](#contributing)


## Introduction

Surf-vcr records HTTP sessions to a YAML file so you can review and modify (or
even create) the sessions manually. A simple recording might look like:

```yml
---
- Request:
    method: POST
    url: "http://localhost:8000/v1/auth/logon"
    headers:
      content-type:
        - text/plain;charset=utf-8
    body: name=Favorite Person&passwd=favorite
- Response:
    status: 200
    version: ~
    headers:
      content-type:
        - text/plain;charset=utf-8
      date:
        - "Fri, 28 May 2021 00:45:04 GMT"
      content-length:
        - 28
    body: TWRoSDA4S3ZnZzNRaGtZbmVxS1Q=
---
- Request:
    method: GET
    url: "http://localhost:8000/v1/view_something"
    headers:
      authorization:
        - Bearer TWRoSDA4S3ZnZzNRaGtZbmVxS1Q=
      content-type:
        - application/json
    body: Some body
- Response:
    status: 200
    version: ~
    headers:
      content-length:
        - 11
      date:
        - "Fri, 28 May 2021 00:45:06 GMT"
      content-type:
        - application/json
    body: "[something]"
---
- Request:
    method: GET
    url: "http://localhost:8000/v1/auth/logoff"
    headers:
      authorization:
        - Bearer NGpRTHREWDUyV0hGTEZEelpSY2U=
    body: ""
- Response:
    status: 200
    version: ~
    headers:
      content-type:
        - application/octet-stream
      content-length:
        - 0
      date:
        - "Fri, 28 May 2021 00:44:58 GMT"
    body: ""
```


### Install

Surf-vcr is not yet on crates.io, so clone the repository locally and add a
path-based dependency in Cargo.toml. You will typically be using it as a
dev-dependency for your tests:

```toml
[dev-dependencies]

surf-vcr = { path = "../surf-vcr" }
```


### Record

Either in your application or the relevant test, register the middleware with
your application in `Record` mode. You will connect to a functioning server and
record all requests and responses to a file. You can safely record multiple HTTP
sessions (tests) into the same file concurrently, but currently should not both
read to and record from the same file at once.

Surf-vcr must be registered **after** any other middleware that modifies the
`Request` or `Response`; otherwise it will not see their modifications and
cannot record them.

```rust
use surf_vcr::{VcrMiddleware, VcrMode};

let vcr = VcrMiddleware::new(VcrMode::Record, "session-recording.yml").await?;

let client = surf::Client::new()
    .with(some_other_middleware)
    .with(vcr);

// And then make your requests:
let req = surf::get("https://www.example.com")
    .insert_header("X-my-header", "stuff");
client.send(req).await?;
```

Take a look at the [simple](examples/simple.rs) example for more.


### Playback

To replay the server's responses, simply change `VcrMode::Record` to
`VcrMode::Replay`:

```rust
let vcr = VcrMiddleware::new(VcrMode::Replay, "session-recording.yml").await?;

let mut client = surf::Client::new()
    .with(some_other_middleware)
    .with(vcr);

// And then make your requests:
let req = surf::get("https://example.com")
    .insert_header("X-my-header", "stuff");
client.send(req).await?;
```


## License

All source code is licensed under the terms of the
[MPL 2.0 license](LICENSE.txt).


## Contributing

Patches and pull requests are welcome. For major features or breaking changes,
please open a ticket or start a discussion first so we can discuss what you
would like to do.
