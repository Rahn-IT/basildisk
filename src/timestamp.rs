use reqwest::header::CONTENT_TYPE;
use thiserror::Error;
use yasna::models::ObjectIdentifier;

const FREETSA_TSR_URL: &str = "https://freetsa.org/tsr";
const TIMESTAMP_QUERY_CONTENT_TYPE: &str = "application/timestamp-query";
const SHA256_OID: &[u64] = &[2, 16, 840, 1, 101, 3, 4, 2, 1];

pub(crate) struct TimestampResponse {
    pub request: Vec<u8>,
    pub response: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum TimestampError {
    #[error("invalid sha256 digest")]
    InvalidDigest,
    #[error("timestamp request failed: {0}")]
    Request(#[from] reqwest::Error),
}

pub(crate) async fn request_sha256_timestamp(
    digest_hex: &str,
) -> Result<TimestampResponse, TimestampError> {
    let query = create_timestamp_query(digest_hex)?;
    let response = reqwest::Client::new()
        .post(FREETSA_TSR_URL)
        .header(CONTENT_TYPE, TIMESTAMP_QUERY_CONTENT_TYPE)
        .body(query.clone())
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    Ok(TimestampResponse {
        request: query,
        response: response.to_vec(),
    })
}

fn create_timestamp_query(digest_hex: &str) -> Result<Vec<u8>, TimestampError> {
    let digest = decode_sha256_digest(digest_hex)?;

    Ok(yasna::construct_der(|writer| {
        writer.write_sequence(|writer| {
            writer.next().write_i64(1);
            writer.next().write_sequence(|writer| {
                writer.next().write_sequence(|writer| {
                    writer
                        .next()
                        .write_oid(&ObjectIdentifier::from_slice(SHA256_OID));
                    writer.next().write_null();
                });
                writer.next().write_bytes(&digest);
            });
            writer.next().write_bool(true);
        });
    }))
}

fn decode_sha256_digest(digest_hex: &str) -> Result<[u8; 32], TimestampError> {
    if digest_hex.len() != 64 {
        return Err(TimestampError::InvalidDigest);
    }

    let mut digest = [0; 32];
    for (index, chunk) in digest_hex.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        digest[index] = (high << 4) | low;
    }

    Ok(digest)
}

fn hex_value(value: u8) -> Result<u8, TimestampError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(TimestampError::InvalidDigest),
    }
}
