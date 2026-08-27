use std::time::Duration;

use futures_util::{future::select, pin_mut, TryStreamExt};
use serde::de::DeserializeOwned;
use worker::{AbortController, Delay, Error, Fetch, Request, Response, Result};

const RESPONSE_BODY_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn fetch_with_timeout(request: Request, timeout: Duration) -> Result<Option<Response>> {
    let controller = AbortController::default();
    let signal = controller.signal();
    let fetch_request = Fetch::Request(request);
    let fetch = fetch_request.send_with_signal(&signal);
    let delay = Delay::from(timeout);
    pin_mut!(fetch, delay);

    match select(fetch, delay).await {
        futures_util::future::Either::Left((response, _)) => response.map(Some),
        futures_util::future::Either::Right(((), _)) => {
            controller.abort();
            Ok(None)
        }
    }
}

pub async fn read_response_limited(
    response: &mut Response,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let read = async {
        let declared = response
            .headers()
            .get("Content-Length")?
            .and_then(|value| value.parse::<usize>().ok());
        if declared.is_some_and(|length| length == 0 || length > limit) {
            return Ok(None);
        };

        let mut body = Vec::with_capacity(declared.unwrap_or(0).min(limit));
        let mut stream = response.stream()?;
        while let Some(mut chunk) = stream.try_next().await? {
            let Some(length) = body.len().checked_add(chunk.len()) else {
                return Ok(None);
            };
            if length > limit {
                return Ok(None);
            }
            body.append(&mut chunk);
        }
        Ok((!body.is_empty()).then_some(body))
    };
    let timeout = Delay::from(RESPONSE_BODY_TIMEOUT);
    pin_mut!(read, timeout);
    match select(read, timeout).await {
        futures_util::future::Either::Left((body, _)) => body,
        futures_util::future::Either::Right(((), _)) => Err(Error::RustError(
            "upstream response body timed out".to_string(),
        )),
    }
}

pub async fn read_json_limited<T: DeserializeOwned>(
    response: &mut Response,
    limit: usize,
) -> Result<T> {
    let body = read_response_limited(response, limit)
        .await?
        .ok_or_else(|| Error::RustError("upstream response body is empty or too large".into()))?;
    serde_json::from_slice(&body)
        .map_err(|error| Error::RustError(format!("upstream returned invalid JSON: {error}")))
}
