use std::{borrow::Cow, str::FromStr, sync::Arc, time::Duration};

use hyper::{
    body::Incoming,
    header::{HeaderName, HeaderValue},
    Request, Response, StatusCode,
};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use tokio::time::timeout;

use crate::{
    config::{ServerParams, TargetType},
    http_response, load_balancing,
    server::serve_file,
    utils::{self},
};

use super::server_utils::ProxyHandlerBody;

#[tracing::instrument(
    name = "Handler",
    fields(ip = %client_ip),
    skip(req, params, loadbalancer, max_req, client, client_ip, scheme)
)]
pub async fn handler(
    req: Request<Incoming>,
    params: Arc<ServerParams>,
    loadbalancer: Arc<load_balancing::LoadBalancerConfig>,
    max_req: Arc<tokio::sync::Semaphore>,
    client: Arc<Client<HttpConnector, Incoming>>,
    client_ip: String,
    scheme: &str,
) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
    // Use the semaphore to limit the number of requests to the upstream server.
    let _permit = match max_req.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            tracing::error!("503 - Request limit reached");
            // Return a 503 error if the limit is reached.
            return Ok(http_response::service_unavailable());
        }
    };

    // Get the authority and domain from the request.
    let (authority, domain) = match get_authority_and_domain(&req) {
        Ok((authority, domain)) => (authority, domain),
        Err(err) => {
            tracing::error!("{}", err);
            return Ok(http_response::bad_request());
        }
    };

    // Get the path from the request.
    let path = req.uri().path_and_query().map_or("/", |p| p.as_str());
    // Used for logs.
    let source_url = format!("{}://{}{}", scheme, &authority, path);

    tracing::info!("Navigate to {}", &source_url);

    // Redirect to HTTPS if the server has TLS configuration.
    if scheme == "http" {
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
    }

    // Check for redirections.

    let match_url = format!("{}{}", domain, utils::remove_last_slash(path));

    let uri_string: Result<(String, bool), _> = match params.targets.get(match_url.as_str()) {
        // First, check for a strict match.
        Some(target_type) => match target_type {
            TargetType::Location(target) => {
                let location =
                    loadbalancer.balance(&target.id, &target.locations, &target.algo, &client_ip);
                Ok((location, target.serve_files))
            }
            TargetType::Redirection(redirection) => {
                return Ok(Response::builder()
                    .status(redirection.code)
                    .header("Location", redirection.location.clone())
                    .body(ProxyHandlerBody::Empty)
                    .unwrap());
            }
        },
        // If no strict match, check for a match with the path.
        None => {
            let mut uri_path: Option<String> = None;
            let mut serve_files: Option<bool> = None;
            let mut red_code: Option<u16> = None; // Http status code if redirection.
            for (url, target_type) in params.targets.iter().rev() {
                match target_type {
                    TargetType::Location(target) => {
                        if !target.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                            let new_path = match_url.strip_prefix(url);
                            let location = loadbalancer.balance(
                                &target.id,
                                &target.locations,
                                &target.algo,
                                &client_ip,
                            );
                            uri_path = Some(format!(
                                "{}{}",
                                utils::remove_last_slash(&location),
                                new_path.unwrap()
                            ));
                            serve_files = Some(target.serve_files);
                            break;
                        }
                    }
                    TargetType::Redirection(redirection) => {
                        if !redirection.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                            let new_path = match_url.strip_prefix(url);
                            uri_path = Some(format!(
                                "{}{}",
                                utils::remove_last_slash(&redirection.location),
                                new_path.unwrap()
                            ));
                            red_code = Some(redirection.code);
                            break;
                        }
                    }
                }
            }

            match uri_path {
                Some(uri) => {
                    // Return the redirection if it exists.
                    if let Some(code) = red_code {
                        return Ok(Response::builder()
                            .status(code)
                            .header("Location", uri)
                            .body(ProxyHandlerBody::Empty)
                            .unwrap());
                    }
                    // Or use Location
                    Ok((uri, serve_files.unwrap()))
                }
                None => Err(()),
            }
        }
    };

    // Extract parts and body from the request.
    let (mut parts, body) = req.into_parts();

    // Request the targeted server.
    let mut new_req: Request<Incoming> = match uri_string {
        Ok((uri, serve_files)) => {
            if !serve_files {
                // Build the reverse proxy request
                parts.uri = uri.parse().unwrap();
                parts.version = hyper::Version::HTTP_11;
                Request::from_parts(parts, body)
            } else {
                // Serve files. Return directly the response.
                let sf = serve_file::serve_file(&uri).await;
                return Ok(sf);
            }
        }
        Err(_) => return Ok(http_response::internal_server_error()),
    };

    // Add the Host header to the request.
    // Required for HTTP/1.1.
    let nr_authority = new_req.uri().authority().unwrap().to_string();
    new_req.headers_mut().insert(
        HeaderName::from_str("Host").unwrap(),
        HeaderValue::from_str(&nr_authority).unwrap(),
    );
    // Add the X-Forwarded-For header to the request.
    new_req.headers_mut().insert(
        HeaderName::from_str("X-Forwarded-For").unwrap(),
        HeaderValue::from_str(&client_ip).unwrap(),
    );
    // Add the X-Forwarded-Host header to the request.
    new_req.headers_mut().insert(
        HeaderName::from_str("X-Forwarded-Host").unwrap(),
        HeaderValue::from_str(&authority).unwrap(),
    );
    // Add the X-Forwarded-Proto header to the request.
    new_req.headers_mut().insert(
        HeaderName::from_str("X-Forwarded-Proto").unwrap(),
        HeaderValue::from_str(scheme).unwrap(),
    );

    // Destination URL for logs.
    let dest_url = new_req.uri().to_string();

    // Embeding the future in a timeout.
    // If the request is too long, return a 504 error.
    let future = client.request(new_req);
    let pending_future = timeout(Duration::from_secs(params.proxy_timeout), future).await;

    let response: Result<Response<Incoming>, hyper_util::client::legacy::Error>;
    match pending_future {
        // Use the response from the future.
        Ok(res) => {
            response = res;
        }
        // Get the error from the timeout and return a 504 error.
        Err(err) => {
            tracing::debug!("Error: {:?}", err);
            tracing::error!("Gateway timeout | {} -> {}", source_url, dest_url);
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
            tracing::debug!("Error: {:?}", err);
            tracing::error!("Bad Gateway | {} -> {}", source_url, dest_url);
            return Ok(http_response::bad_gateway());
        }
    };
}

fn get_authority_and_domain(
    req: &Request<Incoming>,
) -> Result<(String, Cow<str>), Box<dyn std::error::Error>> {
    // Use authority for HTTP/2
    if let Some(authority) = req.uri().authority() {
        let authority_str = authority.to_string();
        let domain = authority.host();
        return Ok((authority_str, Cow::Borrowed(domain)));
    }

    // Use host header.
    let host_header = req.headers().get("host").ok_or("Missing Host header")?;
    let host_str = host_header
        .to_str()
        .map_err(|_| "Invalid Host header encoding")?;
    let domain = host_str
        .split(':')
        .next()
        .ok_or("Invalid Host header format")?;

    Ok((host_str.to_string(), Cow::Borrowed(domain)))
}
