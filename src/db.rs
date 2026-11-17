use serde::de::DeserializeOwned;
use worker::d1::D1Database;
use wasm_bindgen::JsValue;
use worker::*;

pub async fn first<T: DeserializeOwned>(
    db: &D1Database,
    sql: &str,
    args: &[JsValue],
) -> Result<Option<T>> {
    let stmt = db.prepare(sql);
    let stmt = if args.is_empty() { stmt } else { stmt.bind(args)? };
    stmt.first::<T>(None).await
}

pub async fn all<T: DeserializeOwned>(
    db: &D1Database,
    sql: &str,
    args: &[JsValue],
) -> Result<Vec<T>> {
    let stmt = db.prepare(sql);
    let stmt = if args.is_empty() { stmt } else { stmt.bind(args)? };
    let result = stmt.all().await?;
    result.results::<T>()
}

pub async fn run(db: &D1Database, sql: &str, args: &[JsValue]) -> Result<()> {
    let stmt = db.prepare(sql);
    let stmt = if args.is_empty() { stmt } else { stmt.bind(args)? };
    stmt.run().await?;
    Ok(())
}
