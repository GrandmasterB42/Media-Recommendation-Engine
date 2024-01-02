use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

pub struct TemplatingEngine {
    templates: Arc<Mutex<HashMap<String, String>>>,
}

impl TemplatingEngine {
    pub fn new() -> Self {
        Self {
            templates: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
