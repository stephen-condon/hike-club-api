use crate::models::HikeRecord;
use crate::r2::{HikeStore, R2Config, presign_get_url};
use chrono::{DateTime, Utc};

pub struct R2HikeStore<'a> {
    pub bucket: worker::Bucket,
    pub config: &'a R2Config,
}

impl<'a> HikeStore for R2HikeStore<'a> {
    async fn get_hike(&self, id: &str) -> Result<Option<HikeRecord>, String> {
        let key = format!("hikes/{id}.json");
        let object = self
            .bucket
            .get(&key)
            .execute()
            .await
            .map_err(|e| e.to_string())?;
        let Some(object) = object else {
            return Ok(None);
        };
        let bytes = object
            .body()
            .ok_or_else(|| "R2 object had no body".to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    async fn presign_map_url(&self, map_key: &str) -> Result<(String, DateTime<Utc>), String> {
        let now = DateTime::from_timestamp_millis(worker::Date::now().as_millis() as i64)
            .ok_or_else(|| "invalid system time".to_string())?;
        let url = presign_get_url(
            now,
            &self.config.account_id,
            &self.config.bucket,
            map_key,
            &self.config.access_key_id,
            &self.config.secret_access_key,
            self.config.presign_ttl_secs,
        );
        let expires_at = now + chrono::Duration::seconds(self.config.presign_ttl_secs as i64);
        Ok((url, expires_at))
    }
}

pub fn load_r2_config(env: &worker::Env) -> Result<R2Config, String> {
    Ok(R2Config {
        account_id: env
            .var("R2_ACCOUNT_ID")
            .map_err(|e| e.to_string())?
            .to_string(),
        bucket: env
            .var("R2_BUCKET_NAME")
            .map_err(|e| e.to_string())?
            .to_string(),
        access_key_id: env
            .secret("R2_ACCESS_KEY_ID")
            .map_err(|e| e.to_string())?
            .to_string(),
        secret_access_key: env
            .secret("R2_SECRET_ACCESS_KEY")
            .map_err(|e| e.to_string())?
            .to_string(),
        presign_ttl_secs: 3600,
    })
}
