//! In "record" mode, retrieves the content at example.com or another site and
//! stores it in a YAML document. In "play" mode, intercepts the HTTP request
//! and provides the pre-recorded response instead of obtaining one from the
//! remote server.
//!
//! Example runs:
//!
//! ```
//! cargo run --example=simple -- record
//! cargo run --example=simple -- record https://example.com/some/where
//! cargo run --example=simple -- play https://example.com/some/where
//! cargo run --example=simple -- play
//! cargo run --example=simple -- play https://example.com/no/where
//! ```

use std::env;

use async_std::task;

use surf;
use surf_vcr::{VcrMiddleware, VcrMode};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 && (args[1] != "record" || args[1] != "play") {
        println!("Usage: {} record|play [URL]", args[0]);
        return;
    }

    let site = if args.len() == 3 { &args[2] } else { "https://example.com" };

    let mode = if args[1] == "record" {
        VcrMode::Record
    } else if args[1] == "play" {
        VcrMode::Replay
    } else {
        panic!()
    };

    task::block_on(async {
        let vcr = VcrMiddleware::new(mode, "simple-recording-example.yml")
            .await.unwrap();

        let client = surf::Client::new().with(vcr);

        let req = surf::get(site)
            .header("User-Agent", "surf-lib-example")
            .build();

        let mut res = client.send(req).await.unwrap();

        if site == "https://example.com" {
            let text = res.body_string().await.unwrap();
            assert!(text.contains("illustrative examples"));
        }

        println!("Status: {}", res.status());
    });
}
