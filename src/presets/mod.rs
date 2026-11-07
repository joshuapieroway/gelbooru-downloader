use crate::database::history::HistoryDb;
use crate::filters::rules::FilterRules;
use crate::utils::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preset {
    pub name: String,
    pub tags: String,
    #[serde(default)]
    pub excluded_tags: Vec<String>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub min_width: Option<u32>,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub min_height: Option<u32>,
    #[serde(default)]
    pub max_height: Option<u32>,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub max_size: Option<u64>,
    #[serde(default)]
    pub min_score: Option<i64>,
    #[serde(default)]
    pub max_score: Option<i64>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default)]
    pub filename_template: Option<String>,
    #[serde(default)]
    pub folder_template: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    100
}

impl Preset {
    pub fn to_filter_rules(&self) -> FilterRules {
        FilterRules {
            ratings: self
                .rating
                .as_ref()
                .map(|r| crate::filters::rules::parse_ratings(r))
                .unwrap_or_default(),
            min_width: self.min_width,
            max_width: self.max_width,
            min_height: self.min_height,
            max_height: self.max_height,
            min_size: self.min_size,
            max_size: self.max_size,
            min_score: self.min_score,
            max_score: self.max_score,
            extensions: self.extensions.clone(),
            include_tags: Vec::new(),
            exclude_tags: self.excluded_tags.clone(),
            min_id: None,
            max_id: None,
        }
    }
}

pub struct PresetManager {
    db: HistoryDb,
}

impl PresetManager {
    pub fn new(db: HistoryDb) -> Self {
        Self { db }
    }

    pub async fn create(&self, preset: Preset) -> Result<()> {
        let data = serde_json::to_string(&preset).map_err(|e| AppError::Config(e.to_string()))?;
        self.db.save_preset(&preset.name, &data).await
    }

    pub async fn get(&self, name: &str) -> Result<Option<Preset>> {
        if let Some(data) = self.db.get_preset(name).await? {
            let p: Preset =
                serde_json::from_str(&data).map_err(|e| AppError::Config(e.to_string()))?;
            Ok(Some(p))
        } else {
            Ok(None)
        }
    }

    pub async fn list(&self) -> Result<Vec<Preset>> {
        let rows = self.db.list_presets().await?;
        let mut out = Vec::new();
        for (_name, data) in rows {
            if let Ok(p) = serde_json::from_str::<Preset>(&data) {
                out.push(p);
            }
        }
        Ok(out)
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        self.db.delete_preset(name).await
    }

    pub async fn update(&self, preset: Preset) -> Result<()> {
        self.create(preset).await
    }
}
