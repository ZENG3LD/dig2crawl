use crawl_core::error::StorageError;
use rusqlite::Connection;
use std::io::Write;

/// Export all records for `job_id` as newline-delimited JSON (one JSON object per line).
/// Returns the number of records written.
pub fn export_jsonl(
    conn: &Connection,
    job_id: &str,
    writer: &mut dyn Write,
) -> Result<usize, StorageError> {
    let mut stmt = conn
        .prepare("SELECT data_json FROM records WHERE job_id = ?1")
        .map_err(|e| StorageError::Database(format!("export prepare: {e}")))?;
    let rows = stmt
        .query_map([job_id], |row| row.get::<_, String>(0))
        .map_err(|e| StorageError::Database(format!("export query: {e}")))?;
    let mut count = 0;
    for row in rows {
        let json = row.map_err(|e| StorageError::Database(format!("export row: {e}")))?;
        writeln!(writer, "{json}")?;
        count += 1;
    }
    Ok(count)
}

/// Export all records for `job_id` as a pretty-printed JSON array.
/// Returns the number of records written.
pub fn export_json(
    conn: &Connection,
    job_id: &str,
    writer: &mut dyn Write,
) -> Result<usize, StorageError> {
    let mut stmt = conn
        .prepare("SELECT data_json FROM records WHERE job_id = ?1")
        .map_err(|e| StorageError::Database(format!("export prepare: {e}")))?;
    let rows = stmt
        .query_map([job_id], |row| row.get::<_, String>(0))
        .map_err(|e| StorageError::Database(format!("export query: {e}")))?;
    let mut items: Vec<serde_json::Value> = Vec::new();
    for row in rows {
        let json = row.map_err(|e| StorageError::Database(format!("export row: {e}")))?;
        let val: serde_json::Value = serde_json::from_str(&json)?;
        items.push(val);
    }
    let count = items.len();
    serde_json::to_writer_pretty(writer, &items)?;
    Ok(count)
}

/// Export all records for `job_id` as CSV with columns: url, data, confidence, extracted_at.
/// Returns the number of records written.
pub fn export_csv(
    conn: &Connection,
    job_id: &str,
    writer: &mut dyn Write,
) -> Result<usize, StorageError> {
    let mut stmt = conn
        .prepare(
            "SELECT url, data_json, confidence, extracted_at FROM records WHERE job_id = ?1",
        )
        .map_err(|e| StorageError::Database(format!("export prepare: {e}")))?;
    let rows = stmt
        .query_map([job_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| StorageError::Database(format!("export query: {e}")))?;

    let mut csv_writer = csv::Writer::from_writer(writer);
    csv_writer
        .write_record(["url", "data", "confidence", "extracted_at"])
        .map_err(|e| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

    let mut count = 0;
    for row in rows {
        let (url, data, conf, at) =
            row.map_err(|e| StorageError::Database(format!("export row: {e}")))?;
        let conf_str = conf.map(|c| format!("{c:.2}")).unwrap_or_default();
        csv_writer
            .write_record([&url, &data, &conf_str, &at])
            .map_err(|e| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;
        count += 1;
    }
    csv_writer.flush().map_err(|e| {
        StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))
    })?;
    Ok(count)
}
