use async_zip::base::read::mem::ZipFileReader;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, State},
    response::Html,
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use crate::{AppState, error::AppError, users};

const TIMESTAMP_REQUEST_PREFIX: &str = "TimestampRequestBase64: ";
const TIMESTAMP_RESPONSE_PREFIX: &str = "TimestampResponseBase64: ";

#[derive(Serialize)]
struct ImportView {
    is_admin: bool,
    has_result: bool,
    imported: usize,
    skipped_existing: usize,
    skipped_invalid: usize,
    errors: Vec<String>,
}

#[derive(Debug)]
struct ParsedImportedJob {
    id: String,
    disk: String,
    name: String,
    log: String,
    timestamp_request: Option<Vec<u8>>,
    timestamp_response: Option<Vec<u8>>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs/import", get(import_get).post(import_post))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
}

async fn import_get(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
) -> Result<Html<String>, AppError> {
    render_import(
        &state,
        ImportView {
            is_admin: current_user.is_admin,
            has_result: false,
            imported: 0,
            skipped_existing: 0,
            skipped_invalid: 0,
            errors: Vec::new(),
        },
    )
}

async fn import_post(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    mut multipart: Multipart,
) -> Result<Html<String>, AppError> {
    let Some(zip_bytes) = read_zip_upload(&mut multipart).await? else {
        return Err(AppError::conflict("Choose a ZIP file to import."));
    };

    let result = import_zip(&state, zip_bytes).await?;
    render_import(
        &state,
        ImportView {
            is_admin: current_user.is_admin,
            has_result: true,
            imported: result.imported,
            skipped_existing: result.skipped_existing,
            skipped_invalid: result.skipped_invalid,
            errors: result.errors,
        },
    )
}

fn render_import(state: &AppState, view: ImportView) -> Result<Html<String>, AppError> {
    let template = state
        .jinja
        .get_template("job_import.html")
        .expect("template is loaded");
    let rendered = template.render(view)?;
    Ok(Html(rendered))
}

async fn read_zip_upload(multipart: &mut Multipart) -> Result<Option<Vec<u8>>, AppError> {
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("archive") {
            return Ok(Some(field.bytes().await?.to_vec()));
        }
    }

    Ok(None)
}

#[derive(Default)]
struct ImportResult {
    imported: usize,
    skipped_existing: usize,
    skipped_invalid: usize,
    errors: Vec<String>,
}

async fn import_zip(state: &AppState, zip_bytes: Vec<u8>) -> Result<ImportResult, AppError> {
    let zip = ZipFileReader::new(zip_bytes).await?;
    let entries = zip.file().entries().to_vec();
    let mut result = ImportResult::default();

    for (index, entry) in entries.iter().enumerate() {
        if entry.dir().unwrap_or(false) {
            continue;
        }

        let filename = entry.filename().as_str().unwrap_or_default();
        let Some(id) = id_from_filename(filename) else {
            result.skipped_invalid += 1;
            result
                .errors
                .push(format!("Skipped {filename}: expected a UUID .txt file."));
            continue;
        };

        if job_exists(state, &id).await? {
            result.skipped_existing += 1;
            continue;
        }

        let mut reader = zip.reader_with_entry(index).await?;
        let mut txt = String::new();
        reader.read_to_string_checked(&mut txt).await?;

        match parse_imported_job(&id, &txt) {
            Some(job) => {
                insert_imported_job(state, job).await?;
                result.imported += 1;
            }
            None => {
                result.skipped_invalid += 1;
                result
                    .errors
                    .push(format!("Skipped {filename}: could not parse job log."));
            }
        }
    }

    Ok(result)
}

fn id_from_filename(filename: &str) -> Option<String> {
    let filename = filename.rsplit('/').next().unwrap_or(filename);
    let id = filename.strip_suffix(".txt")?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id.to_string())
}

async fn job_exists(state: &AppState, id: &str) -> Result<bool, AppError> {
    let exists = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM jobs WHERE id = $1
        ) as "exists!: bool"
        "#,
        id
    )
    .fetch_one(&state.db)
    .await?;

    Ok(exists)
}

async fn insert_imported_job(state: &AppState, job: ParsedImportedJob) -> Result<(), AppError> {
    sqlx::query!(
        "INSERT INTO jobs (id, disk, name, log, timestamp_request, timestamp_response) VALUES ($1, $2, $3, $4, $5, $6)",
        job.id,
        job.disk,
        job.name,
        job.log,
        job.timestamp_request,
        job.timestamp_response
    )
    .execute(&state.db)
    .await?;

    Ok(())
}

fn parse_imported_job(id: &str, txt: &str) -> Option<ParsedImportedJob> {
    let (log, timestamp_request, timestamp_response) = split_timestamp_payload(txt)?;
    let disk = extract_field(&log, "Device Name:").unwrap_or_else(|| "imported".to_string());
    let model = extract_field(&log, "Model:").unwrap_or_else(|| "Imported Disk".to_string());
    let serial = extract_field(&log, "Serial:").unwrap_or_else(|| "Unknown Serial".to_string());
    let method = extract_field(&log, "Selected Erasure Method:")
        .unwrap_or_else(|| "Imported Job".to_string());
    let name = format!("{method} for {model}: {serial}");

    Some(ParsedImportedJob {
        id: id.to_string(),
        disk,
        name,
        log,
        timestamp_request,
        timestamp_response,
    })
}

fn split_timestamp_payload(txt: &str) -> Option<(String, Option<Vec<u8>>, Option<Vec<u8>>)> {
    let marker = format!("\n{TIMESTAMP_REQUEST_PREFIX}");
    let Some(payload_start) = txt.find(&marker) else {
        return Some((txt.to_string(), None, None));
    };

    let log = txt[..payload_start].to_string();
    let payload = &txt[payload_start + 1..];
    let request = extract_prefixed_line(payload, TIMESTAMP_REQUEST_PREFIX)
        .and_then(|value| STANDARD.decode(value).ok());
    let response = extract_prefixed_line(payload, TIMESTAMP_RESPONSE_PREFIX)
        .and_then(|value| STANDARD.decode(value).ok());

    Some((log, request, response))
}

fn extract_field(log: &str, prefix: &str) -> Option<String> {
    extract_prefixed_line(log, prefix).map(str::to_string)
}

fn extract_prefixed_line<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix).map(str::trim))
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn parses_uuid_txt_filename() {
        assert_eq!(id_from_filename(ID.to_owned().as_str()), None);
        assert_eq!(id_from_filename(&format!("{ID}.txt")).as_deref(), Some(ID));
        assert_eq!(
            id_from_filename(&format!("folder/{ID}.txt")).as_deref(),
            Some(ID)
        );
    }

    #[test]
    fn extracts_timestamp_payload_and_keeps_signature_log() {
        let request = STANDARD.encode([1, 2, 3]);
        let response = STANDARD.encode([4, 5, 6]);
        let txt = format!(
            "Model: M\nSerial: S\nDevice Name: sda\nSelected Erasure Method: ATA Secure Erase\nSHA256: abc\nTimestamp: RFC3161 FreeTSA response available\nTimestampRequestBase64: {request}\nTimestampResponseBase64: {response}\nSignature explanation:\nignored\n"
        );

        let job = parse_imported_job(ID, &txt).unwrap();

        assert_eq!(job.id, ID);
        assert_eq!(job.disk, "sda");
        assert_eq!(job.name, "ATA Secure Erase for M: S");
        assert!(job.log.contains("SHA256: abc"));
        assert!(!job.log.contains("TimestampRequestBase64"));
        assert_eq!(job.timestamp_request.as_deref(), Some([1, 2, 3].as_slice()));
        assert_eq!(
            job.timestamp_response.as_deref(),
            Some([4, 5, 6].as_slice())
        );
    }
}
