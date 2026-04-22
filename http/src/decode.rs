//! Response body decompression utilities.
//!
//! Handles gzip/deflate/br Content-Encoding transparently.
//! Use `decode_response()` to wrap a Response with auto-decompression.

use bytes::Bytes;
use http::Response;
use http_body_util::BodyStream;
use hyper::body::Incoming;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;
use async_compression::tokio::bufread::{GzipDecoder, DeflateDecoder, BrotliDecoder};
use futures_util::stream::{Stream, StreamExt};

/// Error type for decoded body operations.
pub type DecodeError = Box<dyn std::error::Error + Send + Sync>;

/// A decoded response body — either passthrough or decompressed.
pub struct DecodedBody {
    inner: Pin<Box<dyn Stream<Item = Result<http_body::Frame<Bytes>, DecodeError>> + Send>>,
}

impl std::fmt::Debug for DecodedBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedBody").finish()
    }
}

impl http_body::Body for DecodedBody {
    type Data = Bytes;
    type Error = DecodeError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Decode a response body based on Content-Encoding header.
/// Removes the Content-Encoding and Content-Length headers from the response.
/// If no encoding or unknown encoding, passes through unchanged.
pub fn decode_response(resp: Response<Incoming>) -> Response<DecodedBody> {
    let encoding = resp
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let (mut parts, body) = resp.into_parts();

    // Remove content-encoding since we're decoding
    parts.headers.remove("content-encoding");
    // Remove content-length since decoded size is different
    parts.headers.remove("content-length");

    if encoding.contains("gzip") {
        let reader = body_to_async_read(body);
        let decoder = GzipDecoder::new(tokio::io::BufReader::new(reader));
        let stream = tokio_util::io::ReaderStream::new(decoder).map(|r| {
            r.map(|b| http_body::Frame::data(b))
                .map_err(|e| -> DecodeError { Box::new(e) })
        });
        Response::from_parts(parts, DecodedBody { inner: Box::pin(stream) })
    } else if encoding.contains("deflate") {
        let reader = body_to_async_read(body);
        let decoder = DeflateDecoder::new(tokio::io::BufReader::new(reader));
        let stream = tokio_util::io::ReaderStream::new(decoder).map(|r| {
            r.map(|b| http_body::Frame::data(b))
                .map_err(|e| -> DecodeError { Box::new(e) })
        });
        Response::from_parts(parts, DecodedBody { inner: Box::pin(stream) })
    } else if encoding.contains("br") {
        let reader = body_to_async_read(body);
        let decoder = BrotliDecoder::new(tokio::io::BufReader::new(reader));
        let stream = tokio_util::io::ReaderStream::new(decoder).map(|r| {
            r.map(|b| http_body::Frame::data(b))
                .map_err(|e| -> DecodeError { Box::new(e) })
        });
        Response::from_parts(parts, DecodedBody { inner: Box::pin(stream) })
    } else {
        // No encoding or unknown — passthrough
        let stream = BodyStream::new(body).map(|r| {
            r.map(|frame| {
                if let Ok(data) = frame.into_data() {
                    http_body::Frame::data(data)
                } else {
                    http_body::Frame::data(Bytes::new())
                }
            })
            .map_err(|e| -> DecodeError { Box::new(e) })
        });
        Response::from_parts(parts, DecodedBody { inner: Box::pin(stream) })
    }
}

/// Convert Incoming body to AsyncRead for decompression
fn body_to_async_read(body: Incoming) -> impl AsyncRead {
    let stream = BodyStream::new(body).filter_map(|result| async move {
        match result {
            Ok(frame) => frame.into_data().ok().map(Ok),
            Err(e) => Some(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
        }
    });
    StreamReader::new(stream)
}
