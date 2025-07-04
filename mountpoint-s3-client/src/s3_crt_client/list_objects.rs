use std::ops::Deref;
use std::os::unix::prelude::OsStrExt;
use std::str::FromStr;

use mountpoint_s3_crt::http::request_response::Header;
use mountpoint_s3_crt::s3::client::MetaRequestResult;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::error;

use crate::object_client::{
    ChecksumAlgorithm, ListObjectsError, ListObjectsResult, ObjectClientError, ObjectClientResult, ObjectInfo,
    RestoreStatus,
};

use super::{QueryFragment, S3CrtClient, S3Operation, S3RequestError};

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ParseError {
    #[error("XML response was not valid: problem = {1}, xml node = {0:?}")]
    InvalidResponse(Box<xmltree::Element>, String),

    #[error("XML parsing error: {0:?}")]
    Xml(#[from] xmltree::ParseError),

    #[error("Missing field {1} from XML element {0:?}")]
    MissingField(Box<xmltree::Element>, String),

    #[error("Failed to parse field {1} as bool: {0:?}")]
    Bool(#[source] std::str::ParseBoolError, String),

    #[error("Failed to parse field {1} as int: {0:?}")]
    Int(#[source] std::num::ParseIntError, String),

    #[error("Failed to parse field {1} as OffsetDateTime: {0:?}")]
    OffsetDateTime(#[source] time::error::Parse, String),
}

/// Copy text out of an XML element, with the right error type.
fn get_text(element: &xmltree::Element) -> Result<String, ParseError> {
    Ok(element
        .get_text()
        .ok_or_else(|| ParseError::InvalidResponse(element.clone().into(), "field has no text".to_string()))?
        .to_string())
}

/// Wrapper to get child with some name out of an XML element, with the right error type.
fn get_child<'a>(element: &'a xmltree::Element, name: &str) -> Result<&'a xmltree::Element, ParseError> {
    element
        .get_child(name)
        .ok_or_else(|| ParseError::MissingField(element.clone().into(), name.to_string()))
}

/// Get the text out of a child node, with the right error type.
fn get_field(element: &xmltree::Element, name: &str) -> Result<String, ParseError> {
    get_text(get_child(element, name)?)
}

fn parse_result_from_bytes(bytes: &[u8]) -> Result<ListObjectsResult, ParseError> {
    parse_result_from_xml(&mut xmltree::Element::parse(bytes)?)
}

fn parse_result_from_xml(element: &mut xmltree::Element) -> Result<ListObjectsResult, ParseError> {
    let mut objects = Vec::new();

    while let Some(mut content) = element.take_child("Contents") {
        objects.push(parse_object_info_from_xml(&mut content)?);
    }

    let mut common_prefixes = Vec::new();

    while let Some(common_prefix) = element.take_child("CommonPrefixes") {
        let prefix = get_field(&common_prefix, "Prefix")?;
        common_prefixes.push(prefix);
    }

    let mut next_continuation_token = None;
    if let Some(elem) = element.get_child("NextContinuationToken") {
        next_continuation_token = Some(get_text(elem)?);
    }

    let is_truncated = get_field(element, "IsTruncated")?;
    let is_truncated = bool::from_str(&is_truncated).map_err(|e| ParseError::Bool(e, "IsTruncated".to_string()))?;

    if is_truncated != next_continuation_token.is_some() {
        return Err(ParseError::InvalidResponse(
            element.clone().into(),
            "IsTruncated doesn't match NextContinuationToken".to_string(),
        ));
    }

    Ok(ListObjectsResult {
        objects,
        common_prefixes,
        next_continuation_token,
    })
}

fn parse_restore_status(element: &xmltree::Element) -> Result<Option<RestoreStatus>, ParseError> {
    let Some(restore_status) = element.get_child("RestoreStatus") else {
        return Ok(None);
    };

    let restore_in_progress = bool::from_str(&get_field(restore_status, "IsRestoreInProgress")?)
        .map_err(|e| ParseError::Bool(e, "IsRestoreInProgress".to_string()))?;
    if restore_in_progress {
        return Ok(Some(RestoreStatus::InProgress));
    }

    Ok(Some(RestoreStatus::Restored {
        expiry: OffsetDateTime::parse(&get_field(restore_status, "RestoreExpiryDate")?, &Rfc3339)
            .map_err(|e| ParseError::OffsetDateTime(e, "RestoreExpiryDate".to_string()))?
            .into(),
    }))
}

fn parse_checksum_algorithm(element: &mut xmltree::Element) -> Result<Vec<ChecksumAlgorithm>, ParseError> {
    // We expect there only to be at most one algorithm.
    let mut algorithms = Vec::with_capacity(1);

    while let Some(content) = element.take_child("ChecksumAlgorithm") {
        let algo_string = get_text(&content)?;
        let checksum_algorithm = match algo_string.as_str() {
            "CRC64NVME" => ChecksumAlgorithm::Crc64nvme,
            "CRC32" => ChecksumAlgorithm::Crc32,
            "CRC32C" => ChecksumAlgorithm::Crc32c,
            "SHA1" => ChecksumAlgorithm::Sha1,
            "SHA256" => ChecksumAlgorithm::Sha256,
            _ => ChecksumAlgorithm::Unknown(algo_string),
        };
        algorithms.push(checksum_algorithm);
    }

    Ok(algorithms)
}

fn parse_object_info_from_xml(element: &mut xmltree::Element) -> Result<ObjectInfo, ParseError> {
    let key = get_field(element, "Key")?;

    let size = get_field(element, "Size")?;

    let size = u64::from_str(&size).map_err(|e| ParseError::Int(e, "Size".to_string()))?;

    let last_modified = get_field(element, "LastModified")?;

    // S3 appears to use RFC 3339 to encode this field, based on the API example here:
    // https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html
    let last_modified = OffsetDateTime::parse(&last_modified, &Rfc3339)
        .map_err(|e| ParseError::OffsetDateTime(e, "LastModified".to_string()))?;

    let storage_class = get_field(element, "StorageClass").ok();

    let restore_status = parse_restore_status(element)?;

    let etag = get_field(element, "ETag")?;

    let checksum_algorithms = parse_checksum_algorithm(element)?;

    Ok(ObjectInfo {
        key,
        size,
        last_modified,
        storage_class,
        restore_status,
        etag,
        checksum_algorithms,
    })
}

impl S3CrtClient {
    pub(super) async fn list_objects(
        &self,
        bucket: &str,
        continuation_token: Option<&str>,
        delimiter: &str,
        max_keys: usize,
        prefix: &str,
    ) -> ObjectClientResult<ListObjectsResult, ListObjectsError, S3RequestError> {
        // Scope the endpoint, message, etc. since otherwise rustc thinks we use Message across the await.
        let body = {
            let mut message = self
                .inner
                .new_request_template("GET", bucket)
                .map_err(S3RequestError::construction_failure)?;
            message
                .set_header(&Header::new("x-amz-optional-object-attributes", "RestoreStatus"))
                .map_err(S3RequestError::construction_failure)?;
            let max_keys = format!("{max_keys}");
            let mut query = vec![
                ("list-type", "2"),
                ("delimiter", delimiter),
                ("max-keys", &max_keys),
                ("prefix", prefix),
            ];
            if let Some(continuation_token) = continuation_token {
                query.push(("continuation-token", continuation_token));
            }

            message
                .set_request_path_and_query("/", QueryFragment::Query(&query))
                .map_err(S3RequestError::construction_failure)?;

            let span = request_span!(
                self.inner,
                "list_objects",
                bucket,
                continued = continuation_token.is_some(),
                delimiter,
                max_keys,
                prefix
            );

            self.inner.meta_request_with_body_payload(
                message.into_options(S3Operation::ListObjects),
                span,
                parse_list_objects_error,
            )?
        };

        let body = body.await?;

        parse_result_from_bytes(&body)
            .map_err(|e| ObjectClientError::ClientError(S3RequestError::InternalError(e.into())))
    }
}

fn parse_list_objects_error(result: &MetaRequestResult) -> Option<ListObjectsError> {
    match result.response_status {
        404 => {
            let body = result.error_response_body.as_ref()?;
            let root = xmltree::Element::parse(body.as_bytes()).ok()?;
            let error_code = root.get_child("Code")?;
            let error_str = error_code.get_text()?;
            match error_str.deref() {
                "NoSuchBucket" => Some(ListObjectsError::NoSuchBucket),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use super::*;

    fn make_result(response_status: i32, body: impl Into<OsString>) -> MetaRequestResult {
        MetaRequestResult {
            response_status,
            crt_error: 1i32.into(),
            error_response_headers: None,
            error_response_body: Some(body.into()),
        }
    }

    #[test]
    fn parse_404_no_such_bucket() {
        let body = br#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message><BucketName>amzn-s3-demo-bucket</BucketName><RequestId>4YAYHJ0E82DDDNF0</RequestId><HostId>Ajn9+i3d3VWQi339YrGqBbJqQlj5HaX2vplXp9IlDPAxsJ4vsIAsje0P2gJ0of/mTKKz/fv9pNy9RqhbLUBc/g==</HostId></Error>"#;
        let result = make_result(404, OsStr::from_bytes(&body[..]));
        let result = parse_list_objects_error(&result);
        assert_eq!(result, Some(ListObjectsError::NoSuchBucket));
    }
}
