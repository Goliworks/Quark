use std::{borrow::Cow, str::FromStr, sync::Arc, time::Duration};

use hyper::{
    body::Incoming,
    header::{HeaderName, HeaderValue},
    Request, Response, StatusCode,
};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use tokio::time::timeout;

use crate::{
    config::{ConfigHeaders, ServerParams, TargetType},
    http_response, load_balancing,
    server::{serve_file, server_utils::custom_headers},
    utils::{self},
};

use super::server_utils::ProxyHandlerBody;

pub struct HandlerParams<'a> {
    pub req: Request<Incoming>,
    pub client_ip: String,
    pub scheme: &'a str,
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
        hp: HandlerParams<'_>,
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
        let path = hp.req.uri().path_and_query().map_or("/", |p| p.as_str());
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

        let match_url = format!("{domain}{path}");

        // First, check for a strict targets.
        if let Some(target_type) = &self
            .params
            .strict_targets
            .get(utils::remove_last_slash(&match_url))
        {
            return self
                .strict_match(hp, target_type, authority, source_url)
                .await;
        }

        // Else, check for regular targets.
        match self
            .params
            .targets
            .get(utils::remove_last_slash(&match_url))
        {
            // First, check for a strict match.
            Some(target_type) => {
                self.strict_match(hp, target_type, authority, source_url)
                    .await
            }
            // If no strict match, check for a match with the path.
            None => {
                for (url, target_type) in self.params.targets.iter().rev() {
                    match target_type {
                        TargetType::Location(target) => {
                            if match_url.as_str().starts_with(url.as_str()) {
                                let new_path = match_url.strip_prefix(url);
                                let location = self.loadbalancer.balance(
                                    &target.id,
                                    &target.params.location,
                                    &target.algo,
                                    &hp.client_ip,
                                );
                                let uri_path = format!(
                                    "{}{}",
                                    utils::remove_last_slash(&location),
                                    new_path.unwrap()
                                );
                                return self
                                    .proxy_request(
                                        hp,
                                        uri_path,
                                        &target.params.headers,
                                        authority,
                                        source_url,
                                    )
                                    .await;
                            }
                        }
                        TargetType::FileServer(file_server) => {
                            if match_url.as_str().starts_with(url.as_str()) {
                                let new_path = match_url.strip_prefix(url).unwrap();
                                let location =
                                    utils::remove_last_slash(&file_server.params.location);
                                let mut serve_files = serve_file::serve_file(
                                    location,
                                    new_path,
                                    &source_url,
                                    &file_server.fallback_file,
                                    file_server.forbidden_dir,
                                    file_server.is_fallback_404,
                                )
                                .await;

                                if let Some(response) = &file_server.params.headers.response {
                                    custom_headers(&mut serve_files, response);
                                }

                                return Ok(serve_files);
                            }
                        }
                        TargetType::Redirection(redirection) => {
                            if match_url.as_str().starts_with(url.as_str()) {
                                let new_path = match_url.strip_prefix(url);
                                let uri_path = format!(
                                    "{}{}",
                                    utils::remove_last_slash(&redirection.params.location),
                                    new_path.unwrap()
                                );

                                return Ok(Response::builder()
                                    .status(redirection.code)
                                    .header("Location", uri_path)
                                    .body(ProxyHandlerBody::Empty)
                                    .unwrap());
                            }
                        }
                    }
                }
                // If no match, return a 500 internal error.
                tracing::error!("No match for {}", &source_url);
                return Ok(http_response::internal_server_error());
            }
        }
    }

    async fn strict_match(
        &self,
        hp: HandlerParams<'_>,
        target_type: &TargetType,
        authority: String,
        source_url: String,
    ) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
        match target_type {
            TargetType::Location(target) => {
                let location = self.loadbalancer.balance(
                    &target.id,
                    &target.params.location,
                    &target.algo,
                    &hp.client_ip,
                );
                self.proxy_request(hp, location, &target.params.headers, authority, source_url)
                    .await
            }
            TargetType::FileServer(file_server) => {
                let mut serve_files = serve_file::serve_file(
                    &file_server.params.location,
                    "",
                    &source_url,
                    &file_server.fallback_file,
                    file_server.forbidden_dir,
                    file_server.is_fallback_404,
                )
                .await;

                if let Some(response) = &file_server.params.headers.response {
                    custom_headers(&mut serve_files, response);
                }

                Ok(serve_files)
            }
            TargetType::Redirection(redirection) => Ok(Response::builder()
                .status(redirection.code)
                .header("Location", redirection.params.location.clone())
                .body(ProxyHandlerBody::Empty)
                .unwrap()),
        }
    }

    async fn proxy_request(
        &self,
        hp: HandlerParams<'_>,
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
            HeaderValue::from_str(hp.scheme).unwrap(),
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
