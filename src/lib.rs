use std::{
    collections::HashMap,
    path::PathBuf,
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


/// A record-replay middleware for surf.
///
/// This middleware must be registered to the client after any other middleware
/// that modifies the HTTP request, or those modifications will not be recorded
/// and replayed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VcrMiddleware {
    mode: VcrMode,
    file: PathBuf,

    // For now we store requests and responses for ReplayMode as a pair of vecs;
    // we'll iterate the requests until we find the one we want, and return the
    // corresponding response. TODO: A multimap with the request URL or
    // (method, URL) as the key makes more sense.
    requests: Vec<VcrRequest>,
    responses: Vec<VcrResponse>,
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

                let doc = serde_yaml::to_string(&(request, response))?;

                // Each record is a new YAML document.
                file.write_all(doc.as_bytes()).await?;

                res
            },
            VcrMode::Replay => {
                let pos = self.requests.iter().position(|x| x == &request);

                if pos.is_none() {
                    // Return error? panic?
                    todo!()
                }

                Response::from(&self.responses[pos.unwrap()])
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
        let mut requests = vec![];
        let mut responses = vec![];

        if mode == VcrMode::Replay {
            // TODO: Read in chunks.
            let replays = fs::read_to_string(&recording).await?;

            for replay in replays.split("\n---\n") {
                let (request, response) = serde_yaml::from_str(replay)?;
                requests.push(request);
                responses.push(response);
            }
        }

        Ok(Self { mode, file: recording, requests, responses })
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
    body: Vec<u8>,
}

impl VcrRequest {
    pub async fn from_request(req: &mut Request) -> surf::Result<VcrRequest> {
        let headers = {
            let mut headers = HashMap::new();

            for hdr in req.header_names() {
                let values = req.header(hdr).iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<String>>();

                headers.insert(hdr.to_string(), values);
            }

            headers
        };

        let body = req.take_body().into_bytes().await?;
        // We have to replace the body in our source after the copy.
        req.set_body(body.as_slice());

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
    body: Vec<u8>,
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

        let body = resp.body_bytes().await?;
        // We have to replace the body in our source after the copy.
        resp.set_body(body.as_slice());

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

        response.set_body(resp.body.as_slice());

        Response::from(response)
    }
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
