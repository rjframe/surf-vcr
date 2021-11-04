// This Source Code Form is subject to the terms of the Mozilla Public License,
// v. 2.0. If a copy of the MPL was not distributed with this file, You can
// obtain one at https://mozilla.org/MPL/2.0/.

//! Surf-vcr allows you to record communications with an HTTP server, then
//! inject those pre-recorded responses into Surf HTTP sessions, providing you
//! with reproducible tests.
//!
//! You'll typically be testing the functionality of a function that takes a
//! Surf client as a parameter:
//!
//! ```ignore
//! async fn retrieve_widget_list(client: &surf::Client) -> Result<Vec<Widgets>>
//! {
//!     let req = client.get("http://example.com/see-widgets").unwrap();
//!     // ...
//!     # Ok(vec![])
//! }
//! ```
//!
//! To ensure your tests and your application use the same configuration (other
//! middlewares, the base URL, etc) create your client in a function; your tests
//! can then call that function and wrap the client with the VcrMiddleware:
//!
//! ```ignore
//! // Assuming we also have a middleware to manage our sessions, and another to
//! // automatically retry 5xx responses a number of times:
//!
//! pub fn new_http_client(session: SessionMiddleware) -> surf::Client {
//!     let mut client = surf::Client::new()
//!         .with(session)
//!         .with(RetryMiddleware::<3>);
//!
//!     client.set_base_url(Url::parse(SERVER_URL)
//!         .expect("Unable to parse SERVER_URL as a URL"));
//!
//!     client
//! }
//!
//! #[cfg(test)]
//! pub async fn create_test_client(
//!     mode: VcrMode,
//!     cassette: &'static str,
//!     session: Option<Session>,
//! ) -> surf::Client {
//!     let session = session.or(SessionMiddleware::default());
//!
//!     new_http_client(session)
//!         .with(VcrMiddleware::new(mode, cassette).await.unwrap())
//! }
//! ```
//!
//! Now run the server and record the test:
//!
//! ```ignore
//! #[async_std::test]
//! async fn user_cannot_see_widgets_if_not_logged_on() {
//!     let client = create_test_client(
//!         VcrMode::Record,
//!         "tests/sessions/session-tests.yml",
//!         None
//!     ).await.unwrap();
//!
//!     let widgets = retrieve_widget_list(&client).await;
//!     assert!(widgets.is_err());
//! }
//! ```
//!
//! Change the mode to Replay, and you can run the test without connecting to
//! the server. If the server's output changes in the future, you could either
//! manually adjust the YAML file or delete it and re-record the test (if that's
//! common, it may be convenient to have a global MODE variable, and record or
//! replay everything together).
//!
//! ```ignore
//! #[async_std::test]
//! async fn user_cannot_see_widgets_if_not_logged_on() {
//!     let client = create_test_client(
//!         VcrMode::Replay,
//!         "tests/sessions/session-tests.yml",
//!         None
//!     ).await.unwrap();
//!
//!     let widgets = retrieve_widget_list(&client).await;
//!     assert!(widgets.is_err());
//! }
//! ```


use std::{
    collections::HashMap,
    path::PathBuf,
    fmt,
    io,
};

use async_std::{
    prelude::*,
    sync::RwLock,
    fs,
};

use serde::{Serialize, Deserialize};

use surf::{
    http::{self, Method, Version},
    middleware::{Middleware, Next},
    Client,
    Request, Response,
    StatusCode,
    Url,
};

use once_cell::sync::OnceCell;


// For now we store requests and responses for ReplayMode as a pair of vecs;
// we'll iterate the requests until we find the one we want, and return the
// corresponding response. TODO: A multimap with the request URL or
// (method, URL) as the key makes more sense for large recordings.
type Session = (Vec<VcrRequest>, Vec<VcrResponse>);

// We need to guard our file writes; we're going to lock the data though so that
// we can still search for the desired file. The lock is over the session, but
// we're guarding the file path; we must obtain the lock when reading or writing
// to the file, even if we're ignoring the session.
static CASSETTES: OnceCell<RwLock<HashMap<PathBuf, RwLock::<Option<Session>>>>>
    = OnceCell::new();

type RequestModifier = dyn Fn(&mut VcrRequest) + Send + Sync + 'static;
type ResponseModifier = dyn Fn(&mut VcrResponse) + Send + Sync + 'static;

/// Record and playback HTTP sessions.
///
/// This middleware must be registered to the client after any other middleware
/// that modifies the HTTP request, or those modifications will not be recorded
/// and replayed.
///
/// ```
/// # async fn runtest() -> surf::Result {
/// use surf_vcr::{VcrMiddleware, VcrMode};
///
/// let vcr = VcrMiddleware::new(
///     VcrMode::Replay,
///     "test-sessions/session-recording.yml"
/// ).await?;
/// # let some_other_middleware = VcrMiddleware::new(
/// #     VcrMode::Replay,
/// #     "test-sessions/session-recording.yml"
/// # ).await?;
///
/// let mut client = surf::Client::new()
///     .with(some_other_middleware)
///     .with(vcr);
///
/// // And then make your requests:
/// let req = surf::get("https://example.com")
///     .header("X-my-header", "stuff");
///
/// let resp = client.send(req).await?;
/// # Ok(resp) }
/// ```
///
pub struct VcrMiddleware {
    mode: VcrMode,
    file: PathBuf,
    modify_request: Option<Box<RequestModifier>>,
    modify_response: Option<Box<ResponseModifier>>,
}

#[surf::utils::async_trait]
impl Middleware for VcrMiddleware {
    async fn handle(&self, mut req: Request, client: Client, next: Next<'_>)
    -> surf::Result<Response> {
        let mut request = VcrRequest::from_request(&mut req).await?;
        if let Some(ref modifier) = self.modify_request {
            modifier(&mut request);
        }

        match self.mode {
            VcrMode::Record => {
                let mut res = next.run(req, client).await?;
                let mut response = VcrResponse::try_from_response(&mut res).await?;
                if let Some(ref modifier) = self.modify_response {
                    modifier(&mut response);
                }

                let doc = serde_yaml::to_string(
                    &(
                        SerdeWrapper::Request(request),
                        SerdeWrapper::Response(response)
                    )
                )?;

                let recorders = CASSETTES.get().unwrap().read().await;
                let lock = recorders[&self.file].write().await;

                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.file).await?;

                // Each record is a new YAML document.
                file.write_all(doc.as_bytes()).await?;
                drop(lock);

                Ok(res)
            },
            VcrMode::Replay => {
                let cassettes = CASSETTES.get().unwrap().read().await;
                let sessions = &cassettes[&self.file].read().await;

                let (requests, responses) = sessions.as_ref()
                    .expect(&format!("Missing session: {:?}", self.file));

                match requests.iter().position(|x| x == &request) {
                    Some(pos) => Ok(Response::from(&responses[pos])),
                    None => Err(surf::Error::new(
                        StatusCode::NotFound,
                        VcrError::Lookup(Request::from(request))
                    )),
                }
            }
        }
    }
}

impl VcrMiddleware {
    pub async fn new<P>(mode: VcrMode, recording: P) -> Result<Self, VcrError>
        where P: Into<PathBuf>,
    {
        let recording = recording.into();

        if mode == VcrMode::Replay {
            // Ignore error; we only initialize once.
            let _ = CASSETTES.set(RwLock::new(HashMap::new()));

            let mut cassettes = CASSETTES.get().unwrap().write().await;

            let recording_exists = cassettes.contains_key(&recording)
                && cassettes[&recording].read().await.is_some();

            if ! recording_exists {
                let mut requests = vec![];
                let mut responses = vec![];

                let replays = fs::read_to_string(&recording).await?;

                for replay in replays.split("\n---\n") {
                    let (request, response) = serde_yaml::from_str(replay)?;

                    let req = match request {
                        SerdeWrapper::Request(r) => r,
                        _ => panic!("Invalid request"),
                    };
                    let resp = match response {
                        SerdeWrapper::Response(r) => r,
                        _ => panic!("Invalid response"),
                    };

                    requests.push(req);
                    responses.push(resp);
                }

                cassettes.insert(
                    recording.clone(),
                    RwLock::new(Some((requests, responses)))
                );
            }
        } else { // VcrMode::Record
            // Ignore error; we only initialize once.
            let _ = CASSETTES.set(RwLock::new(HashMap::new()));

            let mut recorders = CASSETTES.get().unwrap().write().await;
            recorders.insert(recording.clone(), RwLock::new(None));
        }

        Ok(Self { mode, file: recording, modify_request: None, modify_response: None })
    }

    pub fn with_modify_request<F>(mut self, modifier: F) -> Self
        where F: Fn(&mut VcrRequest) + Send + Sync + 'static {
        self.modify_request.replace(Box::new(modifier));
        self
    }

    pub fn with_modify_response<F>(mut self, modifier: F) -> Self
        where F: Fn(&mut VcrResponse) + Send + Sync + 'static {
        self.modify_response.replace(Box::new(modifier));
        self
    }
}

// If the body is a valid string, it's much nicer to serialize to it; otherwise
// we serialize to bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Body {
    Bytes(Vec<u8>),
    Str(String),
}

impl From<&[u8]> for Body {
    fn from(bytes: &[u8]) -> Self {
        match std::str::from_utf8(&bytes) {
            Ok(s) => Body::Str(s.to_owned()),
            Err(_) => Body::Bytes(bytes.to_vec()),
        }
    }
}

/// Determines whether the middleware should record the HTTP session or inject
/// pre-recorded responses into the session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VcrMode {
    Record,
    Replay,
}

/// Request to be recorded in cassettes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcrRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Body,
}

impl VcrRequest {
    async fn from_request(req: &mut Request) -> surf::Result<VcrRequest> {
        let headers = {
            let mut headers = HashMap::new();

            for header in req.header_names() {
                let values = req.header(header).iter()
                    // We use as_str() before to_string() to prevent the
                    // unnecessary addition of escape characters, which double
                    // up if we round-trip the request and response
                    // de/serializations.
                    .map(|v| v.as_str().to_string())
                    .collect::<Vec<String>>();

                headers.insert(header.to_string(), values);
            }

            headers
        };

        let orig_body = req.take_body().into_bytes().await?;
        let body = Body::from(orig_body.as_slice());

        // We have to replace the body in our source after the copy.
        req.set_body(orig_body.as_slice());

        Ok(Self {
            method: req.method(),
            url: req.url().to_owned(),
            headers,
            body,
        })
    }
}

impl From<VcrRequest> for Request {
    fn from(req: VcrRequest) -> Request {
        let mut request = http::Request::new(req.method, req.url);

        for name in req.headers.keys() {
            let values = &req.headers[name];

            for value in values.iter() {
                request.append_header(name.as_str(), value);
            }
        }

        match &req.body {
            Body::Bytes(b) => request.set_body(b.as_slice()),
            Body::Str(s) => request.set_body(s.as_str()),
        }

        Request::from(request)
    }
}

/// Response to be recorded in cassettes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcrResponse {
    pub status: StatusCode,
    pub version: Option<Version>,
    pub headers: HashMap<String, Vec<String>>,
    // We may want to use the surf::Body type; for large bodies we could stream
    // from the file instead of storing it in memory.
    pub body: Body,
}

impl VcrResponse {
    async fn try_from_response(resp: &mut Response)
    -> surf::Result<VcrResponse> {
        let headers = {
            let mut headers = HashMap::new();

            for hdr in resp.header_names() {
                let values = resp.header(hdr).iter()
                    // We use as_str() before to_string() to prevent the
                    // unnecessary addition of escape characters, which double
                    // up if we round-trip the request and response
                    // de/serializations.
                    .map(|v| v.as_str().to_string())
                    .collect::<Vec<String>>();

                headers.insert(hdr.to_string(), values);
            }

            headers
        };

        let orig_body = resp.body_bytes().await?;
        let body = Body::from(orig_body.as_slice());

        // We have to replace the body in our source after the copy.
        resp.set_body(orig_body.as_slice());

        Ok(Self {
            status: resp.status(),
            version: resp.version(),
            headers,
            body,
        })
    }
}

impl From<&VcrResponse> for Response {
    fn from(resp: &VcrResponse) -> Response {
        let mut response = http::Response::new(resp.status);
        response.set_version(resp.version);

        for name in resp.headers.keys() {
            let values = &resp.headers[name];

            for value in values.iter() {
                response.append_header(name.as_str(), value);
            }
        }

        match &resp.body {
            Body::Bytes(b) => response.set_body(b.as_slice()),
            Body::Str(s) => response.set_body(s.as_str()),
        }

        Response::from(response)
    }
}

// serde only supports externally-tagged enums, but I want to tag the structs.
// See https://github.com/serde-rs/serde/issues/2007
#[derive(Debug, Deserialize, Serialize)]
enum SerdeWrapper {
    Request(VcrRequest),
    Response(VcrResponse),
}

#[derive(Debug)]
pub enum VcrError {
    File(io::Error),
    Parse(serde_yaml::Error),
    Lookup(surf::Request),
}

impl std::error::Error for VcrError {}

impl fmt::Display for VcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(e) => e.fmt(f),
            Self::Parse(e) => e.fmt(f),
            Self::Lookup(req) =>
                write!(f, "Request not found at {}: {:#?}", req.url(), req),
        }
    }
}

impl From<io::Error> for VcrError {
    fn from(e: io::Error) -> Self { Self::File(e) }
}

impl From<serde_yaml::Error> for VcrError {
    fn from(e: serde_yaml::Error) -> Self { Self::Parse(e) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn read_recording_from_disk() -> Result<(), VcrError> {
        let vcr = VcrMiddleware::new(
            VcrMode::Replay,
            "test-sessions/simple.yml"
        ).await?;

        let mut req_headers = HashMap::new();
        req_headers.insert(
            "X-some-header".to_owned(),
            vec!["hello".to_owned()]
        );

        let req = VcrRequest {
            method: Method::Get,
            url: Url::parse("https://example.com").unwrap(),
            headers: req_headers,
            body: Body::Str("My Request".to_owned()),
        };

        let mut res_headers = HashMap::new();
        res_headers.insert(
            "X-some-header".to_owned(),
            vec!["goodbye".to_owned()]
        );

        let res = VcrResponse {
            status: StatusCode::Ok,
            version: None,
            headers: res_headers,
            body: Body::Str("A Response".to_owned()),
        };

        let cassettes = CASSETTES.get().unwrap().read().await;
        let sessions = &cassettes[&vcr.file].read().await;
        let (requests, responses) = sessions.as_ref().unwrap();

        assert_eq!(req, requests[0]);
        assert_eq!(res, responses[0]);

        Ok(())
    }

    #[async_std::test]
    async fn replay_recorded_communications() -> Result<(), VcrError> {
        let vcr = VcrMiddleware::new(
            VcrMode::Replay,
            "test-sessions/simple.yml"
        ).await?
            .with_modify_request(|res| {
                *res.headers.get_mut("secret-header").unwrap() = vec![String::from("(secret)")];
            });

        let client = surf::Client::new().with(vcr);

        let req = surf::get("https://example.com")
            .header("X-some-header", "another hello")
            .header("secret-header", "sensitive data")
            .build();

        let mut res = client.send(req).await.unwrap();

        let mut res_headers = HashMap::new();
        res_headers.insert(
            "x-some-header".to_owned(),
            vec!["another goodbye".to_owned()]
        );
        res_headers.insert(
            "content-type".to_owned(),
            vec!["text/plain;charset=utf-8".to_owned()]
        );
        res_headers.insert(
            "date".to_owned(),
            vec!["Fri, 28 May 2021 00:44:58 GMT".to_owned()]
        );

        let expected = VcrResponse {
            status: StatusCode::Ok,
            version: None,
            headers: res_headers,
            body: Body::Str("A Response".to_owned()),
        };

        assert_eq!(
            VcrResponse::try_from_response(&mut res).await.unwrap(),
            expected
        );

        Ok(())
    }

    #[async_std::test]
    async fn record_communication_in_write_mode() -> Result<(), VcrError> {
        // To avoid the need for a running server, we're actually using two
        // instances of VcrMiddleware - the one under test, and another to
        // replay a session to be recorded.

        let path = "test-sessions/record-test.yml";

        // Ignore a non-existent file; assume deletion succeeds.
        let _ = async_std::fs::remove_file("test-sessions/record-test.yml")
            .await;

        fn hide_session_key(req: &mut VcrRequest) {
            req.headers.entry("session-key".into()).and_modify(|val| *val = vec!["(some key)".into()]);
        }

        fn hide_cookie(res: &mut VcrResponse) {
            res.headers.entry("Set-Cookie".into()).and_modify(|val| *val = vec!["(erased)".into()]);
        }

        let outer = VcrMiddleware::new(
            VcrMode::Replay,
            "test-sessions/simple.yml",
        ).await?;

        let vcr = VcrMiddleware::new(VcrMode::Record, path).await?
            .with_modify_request(hide_session_key)
            .with_modify_response(hide_cookie);

        let client = surf::Client::new()
            .with(vcr)
            .with(outer);

        let req = surf::get("https://example.com")
            .header("X-some-header", "another hello")
            .header("Content-Type", "application/octet-stream")
            .header("session-key", "00112233445566778899AABBCCDDEEFF")
            .build();

        let mut expected_res = client.send(req).await.unwrap();

        // Now we'll create a client to replay what we just did.
        let client = surf::Client::new()
            .with(VcrMiddleware::new(VcrMode::Replay, path).await?.with_modify_request(hide_session_key));

        let req = surf::get("https://example.com")
            .header("X-some-header", "another hello")
            .header("Content-Type", "application/octet-stream")
            .header("session-key", "ffeeddccbbaa99887766554433221100")
            .build();

        let mut res = client.send(req).await.unwrap();
        let mut modified_res = VcrResponse::try_from_response(&mut res).await.unwrap();
        hide_cookie(&mut modified_res);

        assert_eq!(
            modified_res,
            VcrResponse::try_from_response(&mut expected_res).await.unwrap()
        );

        Ok(())
    }
}
