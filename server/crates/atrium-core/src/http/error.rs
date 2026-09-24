//! Every error the API can return, as RFC 9457 `application/problem+json`.
//!
//! [`ApiError`] is a closed enum. Each variant has one status, one stable
//! `code`, and fixed English `title`, `diagnosis` and `remediation` strings.
//! Nothing from the request, the filesystem, a parser or a library error
//! chain is ever placed in a response body: the only dynamic value is the
//! request id, which Core generated or validated. An uncoded error is not
//! representable.
//!
//! `internal.panic` from the plan's table is not here: the release profile
//! sets `panic = "abort"`, so a panic ends the process (systemd restarts it)
//! rather than becoming a response. Handlers are written not to panic.

use bytes::Bytes;
use http::{header, HeaderValue, Response, StatusCode};
use http_body_util::Full;

/// The closed set of API errors, through M1E.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiError {
    /// The route needs an authenticated device and the request did not
    /// carry a valid device token. Missing, malformed, unknown and revoked
    /// tokens are all this, identically.
    Unauthorized,
    /// `Host` is missing, malformed or not one of this server's names.
    HostRejected,
    /// A cross-origin browser request, on a route that does not allow it.
    OriginRejected,
    /// No such route.
    NotFound,
    /// An `/api/<version>` other than `v1`.
    UnsupportedApiVersion,
    /// The route exists, but not for this method.
    MethodNotAllowed,
    /// The route requires TLS 1.3 and this connection negotiated 1.2.
    TlsVersionRequired,
    /// The route takes no request body and one was sent.
    BodyNotAccepted,
    /// The body is larger than the route allows.
    TooLarge,
    /// The body is not the JSON the route expects.
    InvalidBody,
    /// The body has a field the route does not define.
    UnknownField,
    /// The request did not arrive in time.
    Timeout,
    /// Core is in recovery mode; only the recovery routes answer.
    RecoveryMode,
    /// Core could not build its own response.
    Internal,
    /// A pairing request was refused: the one answer for every reason, so
    /// the refusal is not an oracle (ADR-003, API.md §3).
    PairingRejected,
    /// The body is not declared as `application/json`.
    UnsupportedMediaType,
}

impl ApiError {
    /// Every variant, for the enumeration tests.
    pub const ALL: [ApiError; 16] = [
        ApiError::Unauthorized,
        ApiError::HostRejected,
        ApiError::OriginRejected,
        ApiError::NotFound,
        ApiError::UnsupportedApiVersion,
        ApiError::MethodNotAllowed,
        ApiError::TlsVersionRequired,
        ApiError::BodyNotAccepted,
        ApiError::TooLarge,
        ApiError::InvalidBody,
        ApiError::UnknownField,
        ApiError::Timeout,
        ApiError::RecoveryMode,
        ApiError::Internal,
        ApiError::PairingRejected,
        ApiError::UnsupportedMediaType,
    ];

    /// The HTTP status.
    #[must_use]
    pub fn status(self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::HostRejected => StatusCode::MISDIRECTED_REQUEST,
            Self::OriginRejected | Self::TlsVersionRequired | Self::PairingRejected => {
                StatusCode::FORBIDDEN
            }
            Self::NotFound | Self::UnsupportedApiVersion => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::BodyNotAccepted | Self::InvalidBody | Self::UnknownField => {
                StatusCode::BAD_REQUEST
            }
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Timeout => StatusCode::REQUEST_TIMEOUT,
            Self::RecoveryMode => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The stable machine-readable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "auth.unauthorized",
            Self::HostRejected => "request.host_rejected",
            Self::OriginRejected => "request.origin_rejected",
            Self::NotFound => "request.not_found",
            Self::UnsupportedApiVersion => "request.unsupported_api_version",
            Self::MethodNotAllowed => "request.method_not_allowed",
            Self::TlsVersionRequired => "request.tls_version_required",
            Self::BodyNotAccepted => "validation.body_not_accepted",
            Self::TooLarge => "validation.too_large",
            Self::InvalidBody => "validation.invalid_body",
            Self::UnknownField => "validation.unknown_field",
            Self::Timeout => "request.timeout",
            Self::RecoveryMode => "internal.recovery_mode",
            Self::Internal => "internal.server",
            Self::PairingRejected => "pairing.rejected",
            Self::UnsupportedMediaType => "validation.unsupported_media_type",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Unauthorized => "Authentication required",
            Self::HostRejected => "Unrecognised server name",
            Self::OriginRejected => "Cross-origin request refused",
            Self::NotFound => "Not found",
            Self::UnsupportedApiVersion => "Unsupported API version",
            Self::MethodNotAllowed => "Method not allowed",
            Self::TlsVersionRequired => "TLS 1.3 required",
            Self::BodyNotAccepted => "Request body not accepted",
            Self::TooLarge => "Request body too large",
            Self::InvalidBody => "Invalid request body",
            Self::UnknownField => "Unknown field in request body",
            Self::Timeout => "Request timed out",
            Self::RecoveryMode => "Server in recovery mode",
            Self::Internal => "Internal error",
            Self::PairingRejected => "Pairing refused",
            Self::UnsupportedMediaType => "Unsupported media type",
        }
    }

    fn diagnosis(self) -> &'static str {
        match self {
            Self::Unauthorized => "This request needs a paired device.",
            Self::HostRejected => {
                "The request named a host that is not one of this server's addresses or names."
            }
            Self::OriginRejected => "Requests from other web origins are not accepted here.",
            Self::NotFound => "There is nothing at this path.",
            Self::UnsupportedApiVersion => "This server speaks API version 1 only.",
            Self::MethodNotAllowed => "This path does not accept this method.",
            Self::TlsVersionRequired => "This request must be made over TLS 1.3.",
            Self::BodyNotAccepted => "This request takes no body.",
            Self::TooLarge => "The request body is larger than this request allows.",
            Self::InvalidBody => "The request body could not be read as the expected JSON.",
            Self::UnknownField => "The request body contains a field this request does not define.",
            Self::Timeout => "The request was not received in time.",
            Self::RecoveryMode => {
                "The server found a problem with its state and is reporting it without \
                 serving normally."
            }
            Self::Internal => "The server could not complete the response.",
            Self::PairingRejected => {
                "The server did not accept this pairing attempt. Pairing needs a current \
                 pairing code from the server's console."
            }
            Self::UnsupportedMediaType => "This request's body must be sent as application/json.",
        }
    }

    /// A closed vocabulary the client maps to an action.
    fn remediation(self) -> &'static str {
        match self {
            Self::Unauthorized => "pair_this_device",
            Self::HostRejected => "use_server_address",
            Self::OriginRejected => "use_atrium_client",
            Self::NotFound | Self::MethodNotAllowed | Self::BodyNotAccepted => "none",
            Self::UnsupportedApiVersion => "update_client",
            Self::TlsVersionRequired => "update_client",
            Self::TooLarge => "reduce_request_size",
            Self::InvalidBody | Self::UnknownField | Self::UnsupportedMediaType => "fix_request",
            Self::PairingRejected => "get_pairing_code_at_console",
            Self::RecoveryMode => "restore_at_console",
            Self::Internal | Self::Timeout => "retry_later",
        }
    }

    /// `https://atrium.local/errors/<code with dots and underscores as
    /// hyphens>`, as in `API.md` §4.
    #[must_use]
    pub fn type_uri(self) -> String {
        format!(
            "https://atrium.local/errors/{}",
            self.code().replace(['.', '_'], "-")
        )
    }

    /// The problem document. `request_id` is Core's own validated value.
    #[must_use]
    pub fn body(self, request_id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": self.type_uri(),
            "title": self.title(),
            "status": self.status().as_u16(),
            "code": self.code(),
            "diagnosis": self.diagnosis(),
            "remediation": self.remediation(),
            "requestId": request_id,
        })
    }

    /// The response, before the common headers are added.
    #[must_use]
    pub fn response(self, request_id: &str) -> Response<Full<Bytes>> {
        let body = serde_json::to_vec(&self.body(request_id)).unwrap_or_default();
        let mut response = Response::new(Full::new(Bytes::from(body)));
        *response.status_mut() = self.status();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if self == Self::Unauthorized {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_error_variant_has_a_unique_stable_code() {
        let codes: HashSet<&str> = ApiError::ALL.iter().map(|e| e.code()).collect();
        assert_eq!(codes.len(), ApiError::ALL.len());
        let families = ["auth.", "request.", "validation.", "internal.", "pairing."];
        for error in ApiError::ALL {
            assert!(
                families.iter().any(|f| error.code().starts_with(f)),
                "{} is outside the reserved families",
                error.code()
            );
            assert!(error.status().is_client_error() || error.status().is_server_error());
        }
    }

    #[test]
    fn the_problem_shape_is_exactly_rfc_9457_plus_atrium_fields() {
        for error in ApiError::ALL {
            let body = error.body("3f1c");
            let mut keys: Vec<&str> = body
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "code",
                    "diagnosis",
                    "remediation",
                    "requestId",
                    "status",
                    "title",
                    "type"
                ]
            );
            assert_eq!(body["status"], error.status().as_u16());
            let response = error.response("3f1c");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
        }
    }

    #[test]
    fn nothing_dynamic_but_the_request_id() {
        // Every string except requestId is a compile-time constant or derived
        // from one; the same error twice is byte-identical but for the id.
        for error in ApiError::ALL {
            let a = error.body("aa").to_string().replace("\"aa\"", "\"X\"");
            let b = error.body("bb").to_string().replace("\"bb\"", "\"X\"");
            assert_eq!(a, b);
        }
    }

    #[test]
    fn unauthorized_names_the_scheme() {
        let response = ApiError::Unauthorized.response("ab");
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
    }
}
