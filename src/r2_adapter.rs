use crate::models::{HikeLocation, HikeRecord};
use crate::r2::{HikeStore, LOCATIONS_KEY, R2Config, parse_locations, presign_get_url};
use chrono::{DateTime, Utc};

pub struct R2HikeStore<'a> {
    pub bucket: worker::Bucket,
    pub config: &'a R2Config,
}

impl R2HikeStore<'_> {
    /// The object's bytes, `None` when it is absent, `Err` when it has no body.
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let object = self
            .bucket
            .get(key)
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
        Ok(Some(bytes))
    }
}

// @spec HIKE-OBJ-001, HIKE-OBJ-003, HIKE-REC-001, HIKE-REC-002, HIKE-REC-003, HIKE-MAP-001, HIKE-MAP-002,
// @spec HIKE-MAP-006, HIKE-MAP-009
impl<'a> HikeStore for R2HikeStore<'a> {
    async fn get_hike(&self, id: &str) -> Result<Option<HikeRecord>, String> {
        let Some(bytes) = self.read(&format!("hikes/{id}.json")).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    // @spec HIKE-LOC-001, HIKE-LOC-002
    async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String> {
        let Some(bytes) = self.read(LOCATIONS_KEY).await? else {
            return Ok(None);
        };
        parse_locations(&bytes).map(Some)
    }

    async fn presign_map_url(
        &self,
        map_key: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, String> {
        // Signing is arithmetic and would sign a key that was never uploaded.
        if self
            .bucket
            .head(map_key)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(None);
        }
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
        Ok(Some((url, expires_at)))
    }
}

// @spec HIKE-CFG-001, HIKE-CFG-002, HIKE-MAP-005
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
