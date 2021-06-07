# Surf-vcr - Record and Replay HTTP sessions

Surf-vcr is a testing middleware for the [Surf](https://github.com/http-rs/Surf)
HTTP client library. Surf-vcr records your client's HTTP sessions with a server
to later mock the server's HTTP responses, providing deterministic testing of
your clients.

The high-level design is based on [VCR](https://github.com/vcr/vcr) for Ruby.

Source code is available on [SourceHut](https://git.sr.ht/~rjframe/surf-vcr) and
[Github](https://github.com/rjframe/surf-vcr). Patches may be sent via either
service, but the CI is running on SourceHut.


## Table of Contents

* [Introduction](#introduction)
    * [Install](#application-installation)
    * [Record](#record)
    * [Playback](#playback)
* [License](#license)
* [Contributing](#contributing)


## Introduction

Surf-vcr records HTTP sessions to a YAML file so you can review and modify (or
even create) the requests and responses manually. A simple recording might look
like:

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

You'll typically be using surf-vcr as a development dependency, so add it as
such via Cargo:

```sh
cargo add -D surf-vcr
```

Or add it to your `Cargo.toml` file manually:

```toml
[dev-dependencies]

surf-vcr = "0.1.0"
```


### Record

Either in your application or the relevant test, register the middleware with
your application in `Record` mode. You will connect to a functioning server and
record all requests and responses to a file. You can safely record multiple HTTP
sessions (tests) into the same file concurrently.

Surf-vcr must be registered **after** any other middleware that modifies the
`Request` or `Response`; otherwise it will not see their modifications and
cannot record them.

I have found it useful to use a function in my application to create the Surf
client with my middleware, then call that function in my tests as well so I know
my test client and application client are identical:

```rust
fn create_surf_client() -> surf::Client {
    let session = MySessionMiddleware::new();

    surf::Client::new()
        .with(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::task;
    use surf_vcr::{VcrError, VcrMiddleware, VcrMode};

    async fn create_test_client(mode: VcrMode, cassette: &'static str)
    -> std::result::Result<surf::Client, VcrError>
    {
        let client = create_surf_client()
            .with(VcrMiddleware::new(mode, cassette).await?);

        Ok(client)
    }

    #[async_std::test]
    async fn test_example_request() {
        let client = create_test_client(
            mode::VcrMode::Record,
            "sessions/my-session.yml"
        ).await.unwrap();

        let req = surf::get("https://www.example.com")
            .insert_header("X-my-header", "stuff");

        let mut res = client.send(req).await.unwrap();
        assert_eq!(res.status(), surf::StatusCode::Ok);

        let content = res.body_string().await.unwrap();
        assert!(content.contains("illustrative examples"));
    }
}
```

Take a look at the [simple](examples/simple.rs) example for more.


### Playback

To mock the server's responses simply change `VcrMode::Record` to
`VcrMode::Replay` and re-run your tests. Surf-vcr will look up each request
made, intercept it, and return the saved response.


## License

All source code is licensed under the terms of the
[MPL 2.0 license](LICENSE.txt).


## Contributing

Patches and pull requests are welcome. For major features or breaking changes,
please open a ticket or start a discussion first so we can discuss what you
would like to do.
