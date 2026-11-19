//! Plain HTTP for the two calls the http client does not make: a `whatif` to a mesh's endpoint
//! (CONTRACT-build2-part3-calculus E6) and the publish of a bundle to a public mesh
//! (CONTRACT-build2-part4-publish P1), both JSON. The agent is set up as the http client's: no
//! redirects, status codes are answers, and the timeout is the configured one.

use std::time::Duration;

/// The most bytes an answer may have here.
const LIMIT: u64 = 16 * 1024 * 1024;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .user_agent(concat!("datumesh-local/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// `GET url`: the status and the body.
pub(crate) fn get(url: &str, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let mut resp = agent(timeout).get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().with_config().limit(LIMIT).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
    Ok((status, body))
}

/// `POST url` with a JSON `body`: the status and the body.
pub(crate) fn post(url: &str, body: &[u8], timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let mut resp = agent(timeout)
        .post(url)
        .header("content-type", "application/json")
        .send(body)
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let answer = resp.body_mut().with_config().limit(LIMIT).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
    Ok((status, answer))
}
