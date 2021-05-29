// This Source Code Form is subject to the terms of the Mozilla Public License,
// v. 2.0. If a copy of the MPL was not distributed with this file, You can
// obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::RwLock,
    fmt,
    io,
};

use async_std::{
    prelude::*,
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
static CASSETTES:
    OnceCell<RwLock<HashMap<PathBuf, (Vec<VcrRequest>, Vec<VcrResponse>)>>>
        = OnceCell::new();

/// A record-replay middleware for surf.
///
/// This middleware must be registered to the client after any other middleware
/// that modifies the HTTP request, or those modifications will not be recorded
/// and replayed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VcrMiddleware {
    mode: VcrMode,
    file: PathBuf,
}

#[surf::utils::async_trait]
impl Middleware for VcrMiddleware {
    async fn handle(&self, mut req: Request, client: Client, next: Next<'_>)
    -> surf::Result<Response> {
        let request = VcrRequest::from_request(&mut req).await?;

        let res = match self.mode {
            VcrMode::Record => {
                let mut res = next.run(req, client).await?;
                let response = VcrResponse::try_from_response(&mut res).await?;

                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.file).await?;

                let doc = serde_yaml::to_string(
                    &(
                        SerdeWrapper::Request(request),
                        SerdeWrapper::Response(response)
                    )
                )?;

                // Each record is a new YAML document.
                file.write_all(doc.as_bytes()).await?;

                res
            },
            VcrMode::Replay => {
                let cassettes = CASSETTES.get().unwrap().read().unwrap();

                let (requests, responses) = &cassettes[&self.file];

                match requests.iter().position(|x| x == &request) {
                    Some(pos) => Response::from(&responses[pos]),
                    None => todo!() // Return error? Panic?
                }
            }
        };

        Ok(res)
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

            let mut cassettes = CASSETTES.get().unwrap().write().unwrap();

            if ! cassettes.contains_key(&recording) {
                let mut requests = vec![];
                let mut responses = vec![];

                let replays = fs::read_to_string(&recording).await?;

                for replay in replays.split("\n---\n") {
                    let (request, response) = serde_yaml::from_str(replay)?;

                    // TODO: Return errors; the panics can only occur after
                    // manually incorrectly-editing a file, but will poison the
                    // RwLock causing subsequent tests to fail.
                    let req = match request {
                        SerdeWrapper::Request(r) => r,
                        _ => panic!(),
                    };
                    let resp = match response {
                        SerdeWrapper::Response(r) => r,
                        _ => panic!(),
                    };

                    requests.push(req);
                    responses.push(resp);
                }

                cassettes.insert(recording.clone(), (requests, responses));
            }
        }

        Ok(Self { mode, file: recording })
    }
}

// If the body is a valid string, it's much nicer to serialize to it; otherwise
// we serialize to bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Body {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VcrMode {
    Record,
    Replay,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcrRequest {
    method: Method,
    url: Url,
    headers: HashMap<String, Vec<String>>,
    body: Body,
}

impl VcrRequest {
    pub async fn from_request(req: &mut Request) -> surf::Result<VcrRequest> {
        let headers = {
            let mut headers = HashMap::new();

            for header in req.header_names() {
                let values = req.header(header).iter()
                    .map(|v| v.to_string())
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcrResponse {
    status: StatusCode,
    version: Option<Version>,
    headers: HashMap<String, Vec<String>>,
    // We may want to use the surf::Body type; for large bodies we could stream
    // from the file instead of storing it in memory.
    body: Body,
}

impl VcrResponse {
    pub async fn try_from_response(resp: &mut Response)
    -> surf::Result<VcrResponse> {
        let headers = {
            let mut headers = HashMap::new();

            for hdr in resp.header_names() {
                let values = resp.header(hdr).iter()
                    .map(|v| v.to_string())
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
}

impl std::error::Error for VcrError {}

impl fmt::Display for VcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(e) => e.fmt(f),
            Self::Parse(e) => e.fmt(f),
        }
    }
}

impl From<io::Error> for VcrError {
    fn from(e: io::Error) -> Self { Self::File(e) }
}

impl From<serde_yaml::Error> for VcrError {
    fn from(e: serde_yaml::Error) -> Self { Self::Parse(e) }
}
