use anyhow::Result;
use std::{net::IpAddr, str::FromStr};

use axum::{
    extract::FromRequestParts,
    http::request::Parts,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[allow(dead_code)]
pub struct ExtractIP(pub Option<std::net::IpAddr>);

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for ExtractIP
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let headers = parts.headers.clone();

        cfg_if::cfg_if! {
            if #[cfg(feature = "cloudflare")] {
                match headers.get("CF-Connecting-IP") {
                    Some(ip) => {
                        #[rustfmt::skip]
                        let ip = IpAddr::from_str(ip.to_str().map_err(|err|
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Cannot parse CF-Connecting-IP: {}", err),
                            ).into_response()
                        )?).map_err(|err| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Cannot convert CF-Connecting-IP: {}", err),
                            ).into_response()
                        })?;
                        Ok(Self(Some(ip)))
                    }
                    None => Ok(Self(None)),
                }
            } else if #[cfg(feature = "native")] {
                match headers.get("X-Real-IP") {
                    Some(ip) => {
                        #[rustfmt::skip]
                        let ip = IpAddr::from_str(ip.to_str().map_err(|err|
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Cannot parse X-Real-IP: {}", err),
                            ).into_response()
                        )?).map_err(|err| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Cannot convert X-Real-IP: {}", err),
                            ).into_response()
                        })?;

                        Ok(Self(Some(ip)))
                    }
                    None => Ok(Self(None)),
                }
            } else {
                Ok(Self(None))
            }
        }
    }
}

#[allow(unused)]
pub fn is_internal_ip(ip: IpAddr) -> Result<bool> {
    let ip = match ip {
        IpAddr::V4(ip) => ip,
        IpAddr::V6(_) => {
            // 目前所内内网地址只支持 IPv4
            return Ok(false);
        }
    };

    // 127.0.0.1
    if ip.is_loopback() {
        return Ok(true);
    }

    let octets = ip.octets();
    Ok(match octets[0] {
        10 => true,
        172 => octets[1] >= 16 && octets[1] <= 31,
        192 => octets[1] == 168,
        _ => false,
    })
}
