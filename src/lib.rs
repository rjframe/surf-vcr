use std::{
    collections::HashMap,
    path::PathBuf,
    fs,
};

use serde::{Serialize, Deserialize};

use surf::{
    http::{Method, Version},
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
    async fn handle(&self, req: Request, client: Client, next: Next<'_>)
    -> surf::Result<Response> {
        let request = VcrRequest::from(&req);

        let res = match self.mode {
            VcrMode::Record => {
                let res = next.run(req, client).await?;
                let response = VcrResponse::from(&res);
                // TODO: Append (request, response) to file.
                res
            },
            VcrMode::Replay => {
                // TODO: look up response, return it.
                todo!()
            }
        };

        Ok(res)
    }
}

impl VcrMiddleware {
    pub fn new<P>(mode: VcrMode, recording: P) -> Self
        where P: Into<PathBuf>,
    {
        let mut requests = vec![];
        let mut responses = vec![];

        if mode == VcrMode::Replay {
            // TODO: Open the file, read each YAML document.
        }

        Self { mode, file: recording.into(), requests, responses }
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

impl From<&Request> for VcrRequest {
    fn from(req: &Request) -> VcrRequest {
        todo!()
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

impl From<&Response> for VcrResponse {
    fn from(resp: &Response) -> VcrResponse {
        todo!()
    }
}

impl From<&VcrResponse> for Response {
    fn from(resp: &VcrResponse) -> Response {
        todo!()
    }
}
