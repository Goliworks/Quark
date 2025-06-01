use std::{sync::Arc, time::Duration};

use hyper::{body::Incoming, Request, Response, StatusCode};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use tokio::time::timeout;

use crate::{
    config::ServerParams,
    http_response,
    serve_file::serve_file,
    utils::{self, ProxyHandlerBody},
};

pub async fn proxy_handler(
    req: Request<Incoming>,
    params: Arc<ServerParams>,
    max_req: Arc<tokio::sync::Semaphore>,
    client: Arc<Client<HttpConnector, Incoming>>,
) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
    // Use the semaphore to limit the number of requests to the upstream server.
    let _permit = match max_req.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            // Return a 503 error if the limit is reached.
            return Ok(http_response::service_unavailable());
        }
    };

    // Get the domain.
    // Use authority for HTTP/2
    let domain = if req.uri().authority().is_some() {
        req.uri().authority().unwrap().host()
    } else {
        req.headers()["host"]
            .to_str()
            .unwrap()
            .split(':')
            .next()
            .unwrap()
    };
    // Get the path from the request.
    let path = req.uri().path_and_query().unwrap().as_str();

    // Redirect to HTTPS if the server has TLS configuration.
    if let Some(dom) = params
        .auto_tls
        .as_ref()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|x| x.starts_with(&domain.to_string()))
    {
        return Ok(Response::builder()
            .status(StatusCode::PERMANENT_REDIRECT)
            .header("Location", format!("https://{}{}", dom, path))
            .body(ProxyHandlerBody::Empty)
            .unwrap());
    }

    // Check for redirections.

    let match_url = format!("{}{}", domain, utils::remove_last_slash(path));

    match params.redirections.get(match_url.as_str()) {
        // First, check for a strict match.
        Some(redirection) => {
            return Ok(Response::builder()
                .status(redirection.code)
                .header("Location", redirection.location.clone())
                .body(ProxyHandlerBody::Empty)
                .unwrap());
        }
        // If no strict match, check for a match with the path.
        None => {
            let mut uri_path: Option<String> = None;
            let mut red_code: Option<u16> = None;
            for (url, target) in params.redirections.iter().rev() {
                if !target.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                    let new_path = match_url.strip_prefix(url);
                    uri_path = Some(format!(
                        "{}{}",
                        utils::remove_last_slash(&target.location),
                        new_path.unwrap()
                    ));
                    red_code = Some(target.code);
                    break;
                }
            }

            if let Some(uri) = uri_path {
                return Ok(Response::builder()
                    .status(red_code.unwrap())
                    .header("Location", uri)
                    .body(ProxyHandlerBody::Empty)
                    .unwrap());
            }
        }
    }

    // Get the domain (and remove port) from host.

    let uri_string: Result<(String, bool), _> = match params.targets.get(match_url.as_str()) {
        // First, check for a strict match.
        Some(target) => Ok((target.location.clone(), target.serve_files)),
        // If no strict match, check for a match with the path.
        None => {
            let mut uri_path: Option<String> = None;
            let mut serve_files: Option<bool> = None;
            for (url, target) in params.targets.iter().rev() {
                if !target.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                    let new_path = match_url.strip_prefix(url);
                    uri_path = Some(format!(
                        "{}{}",
                        utils::remove_last_slash(&target.location),
                        new_path.unwrap()
                    ));
                    serve_files = Some(target.serve_files);
                    break;
                }
            }

            match uri_path {
                Some(uri) => Ok((uri, serve_files.unwrap())),
                None => Err(()),
            }
        }
    };

    // Build the client.
    // Extract parts and body from the request.
    let (mut parts, body) = req.into_parts();

    // Request the targeted server.
    let new_req: Request<Incoming> = match uri_string {
        Ok((uri, serve_files)) => {
            if !serve_files {
                // Build the reverse proxy request
                parts.uri = uri.parse().unwrap();
                parts.version = hyper::Version::HTTP_11;
                Request::from_parts(parts, body)
            } else {
                // Serve files. Return directly the response.
                let sf = serve_file(&uri).await;
                return Ok(sf);
            }
        }
        Err(_) => return Ok(http_response::internal_server_error()),
    };

    let future = client.request(new_req);

    // Embeding the future in a timeout.
    // If the request is too long, return a 504 error.
    let pending_future = timeout(Duration::from_secs(params.proxy_timeout), future).await;

    let response: Result<Response<Incoming>, hyper_util::client::legacy::Error>;
    match pending_future {
        // Use the response from the future.
        Ok(res) => {
            response = res;
        }
        // Get the error from the timeout and return a 504 error.
        Err(err) => {
            println!("Error: {:?}", err);
            return Ok(http_response::gateway_timeout());
        }
    };

    // Return the response from the request.
    match response {
        // If the request succeeded, return the response.
        // It's the data from the targeted server.
        Ok(res) => {
            let res = res.map(ProxyHandlerBody::Incoming);
            return Ok(res);
        }
        // If the request failed, return a 502 error.
        Err(err) => {
            println!("Error: {:?}", err);
            return Ok(http_response::bad_gateway());
        }
    };
}
