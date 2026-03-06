use std::{borrow::Cow, str::FromStr, sync::Arc, time::Duration};

use hyper::{
    body::Incoming,
    header::{HeaderName, HeaderValue},
    Request, Response, StatusCode,
};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use tokio::time::timeout;

use crate::{
    config::{ConfigHeaders, RouteKind, ServerParams, TargetType},
    http_response, load_balancing,
    server::{serve_file, server_utils::custom_headers},
    utils::{self},
};

use super::server_utils::ProxyHandlerBody;

enum ResolvedTarget<'a> {
    Proxy {
        uri: String,
        headers: &'a ConfigHeaders,
    },
    File {
        location: &'a str,
        sub_path: &'a str,
        headers: &'a ConfigHeaders,
        fallback_file: &'a Option<String>,
        forbidden_dir: bool,
        is_fallback_404: bool,
    },
    Redirect {
        code: u16,
        location: String,
    },
}

pub struct HandlerParams {
    pub req: Request<Incoming>,
    pub client_ip: String,
    pub scheme: String,
}

pub struct ServerHandler {
    params: Arc<ServerParams>,
    loadbalancer: Arc<load_balancing::LoadBalancerConfig>,
    max_req: Arc<tokio::sync::Semaphore>,
    client: Arc<Client<HttpConnector, Incoming>>,
}

impl ServerHandler {
    pub fn builder(
        params: Arc<ServerParams>,
        loadbalancer: Arc<load_balancing::LoadBalancerConfig>,
        max_req: Arc<tokio::sync::Semaphore>,
        client: Arc<Client<HttpConnector, Incoming>>,
    ) -> Arc<ServerHandler> {
        Arc::new(ServerHandler {
            params,
            loadbalancer,
            max_req,
            client,
        })
    }

    #[tracing::instrument(
    name = "Handler",
    fields(ip = %hp.client_ip),
    skip(self, hp)
    )]
    pub async fn handle(
        &self,
        hp: HandlerParams,
    ) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
        // Use the semaphore to limit the number of requests to the upstream server.
        let _permit = match self.max_req.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tracing::error!("503 - Request limit reached");
                // Return a 503 error if the limit is reached.
                return Ok(http_response::service_unavailable());
            }
        };

        // Get the authority and domain from the request.
        let (authority, domain) = match get_authority_and_domain(&hp.req) {
            Ok((authority, domain)) => (authority, domain),
            Err(err) => {
                tracing::error!("{}", err);
                return Ok(http_response::bad_request());
            }
        };

        // Get the path from the request.
        let path = hp
            .req
            .uri()
            .path_and_query()
            .map_or("/".to_string(), |p| p.as_str().to_string());
        let source_url = format!("{}://{}{}", hp.scheme, &authority, path);

        tracing::info!("Navigate to {}", &source_url);

        // Redirect to HTTPS if the server has TLS configuration.
        if hp.scheme == "http" {
            if let Some(dom) = self
                .params
                .auto_tls
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .find(|x| x.starts_with(&domain.to_string()))
            {
                return Ok(Response::builder()
                    .status(StatusCode::PERMANENT_REDIRECT)
                    .header("Location", format!("https://{dom}{path}"))
                    .body(ProxyHandlerBody::Empty)
                    .unwrap());
            }
        }

        let domain = domain.to_string();
        let path = utils::remove_last_slash(&path);
        let client_ip = hp.client_ip.clone();

        match self.resolve(&domain, path, &client_ip) {
            Some(ResolvedTarget::Proxy { uri, headers }) => {
                self.proxy_request(hp, uri, headers, authority, source_url)
                    .await
            }
            Some(ResolvedTarget::File {
                location,
                sub_path,
                headers,
                fallback_file,
                forbidden_dir,
                is_fallback_404,
            }) => {
                let mut res = serve_file::serve_file(
                    location,
                    sub_path,
                    &source_url,
                    fallback_file,
                    forbidden_dir,
                    is_fallback_404,
                )
                .await;

                if let Some(response) = &headers.response {
                    custom_headers(&mut res, response);
                }

                Ok(res)
            }
            Some(ResolvedTarget::Redirect { code, location }) => Ok(Response::builder()
                .status(code)
                .header("Location", location)
                .body(ProxyHandlerBody::Empty)
                .unwrap()),
            None => {
                // If no match, return a 500 internal error.
                tracing::error!("No match for {}", &source_url);
                Ok(http_response::internal_server_error())
            }
        }
    }

    fn resolve<'a>(
        &'a self,
        domain: &str,
        path: &'a str,
        client_ip: &'a str,
    ) -> Option<ResolvedTarget<'a>> {
        let routes = self.params.routes.get(domain)?;

        for route in routes {
            match route.kind {
                RouteKind::Strict => {
                    if path == route.path {
                        return Some(self.build_resolved(&route.target, "", client_ip));
                    }
                }
                RouteKind::Path => {
                    if path.starts_with(&route.path) {
                        let sub_path = path.strip_prefix(&route.path).unwrap();
                        return Some(self.build_resolved(&route.target, sub_path, client_ip));
                    }
                }
            }
        }
        None
    }

    fn build_resolved<'a>(
        &'a self,
        target_type: &'a TargetType,
        sub_path: &'a str,
        client_ip: &'a str,
    ) -> ResolvedTarget<'a> {
        match target_type {
            TargetType::Location(target) => {
                let location = self.loadbalancer.balance(
                    &target.id,
                    &target.params.location,
                    &target.algo,
                    client_ip,
                );
                let uri = format!("{}{}", utils::remove_last_slash(&location), sub_path);
                ResolvedTarget::Proxy {
                    uri,
                    headers: &target.params.headers,
                }
            }
            TargetType::FileServer(file_server) => ResolvedTarget::File {
                location: utils::remove_last_slash(&file_server.params.location),
                sub_path,
                headers: &file_server.params.headers,
                fallback_file: &file_server.fallback_file,
                forbidden_dir: file_server.forbidden_dir,
                is_fallback_404: file_server.is_fallback_404,
            },
            TargetType::Redirection(redirection) => ResolvedTarget::Redirect {
                code: redirection.code,
                location: format!(
                    "{}{}",
                    utils::remove_last_slash(&redirection.params.location),
                    sub_path
                ),
            },
        }
    }

    async fn proxy_request(
        &self,
        hp: HandlerParams,
        uri: String,
        headers: &ConfigHeaders,
        authority: String,
        source_url: String,
    ) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
        // Extract parts and body from the request.
        let (mut parts, body) = hp.req.into_parts();

        // Request the targeted server.
        let mut new_req: Request<Incoming> = {
            parts.uri = uri.parse().unwrap();
            parts.version = hyper::Version::HTTP_11;
            Request::from_parts(parts, body)
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
            HeaderValue::from_str(&hp.client_ip).unwrap(),
        );
        // Add the X-Forwarded-Host header to the request.
        new_req.headers_mut().insert(
            HeaderName::from_str("X-Forwarded-Host").unwrap(),
            HeaderValue::from_str(&authority).unwrap(),
        );
        // Add the X-Forwarded-Proto header to the request.
        new_req.headers_mut().insert(
            HeaderName::from_str("X-Forwarded-Proto").unwrap(),
            HeaderValue::from_str(&hp.scheme).unwrap(),
        );

        // Add or remove headers defined in the config file.
        if let Some(h) = &headers.request {
            custom_headers(&mut new_req, h);
        }

        // Destination URL for logs.
        let dest_url = new_req.uri().to_string();

        // Embeding the future in a timeout.
        // If the request is too long, return a 504 error.
        let future = self.client.request(new_req);
        let pending_future = timeout(Duration::from_secs(self.params.proxy_timeout), future).await;

        let response = match pending_future {
            // Use the response from the future.
            Ok(res) => res,
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
                let mut res = res.map(ProxyHandlerBody::Incoming);
                // Add or remove headers defined in the config file.
                if let Some(response) = &headers.response {
                    custom_headers(&mut res, response);
                }
                Ok(res)
            }
            // If the request failed, return a 502 error.
            Err(err) => {
                tracing::debug!("Error: {:?}", err);
                tracing::error!("Bad Gateway | {} -> {}", source_url, dest_url);
                Ok(http_response::bad_gateway())
            }
        }
    }
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
