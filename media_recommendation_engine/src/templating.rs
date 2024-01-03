use core::fmt;
use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::utils::{relative_str, HandleErr};

#[derive(Clone)]
pub struct TemplatingEngine {
    pub templates: Arc<Mutex<HashMap<String, Box<dyn std::any::Any + Send + Sync>>>>,
}

impl TemplatingEngine {
    pub fn new() -> Self {
        Self {
            templates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get<T: 'static + Into<isize> + Clone + fmt::Debug>(
        &self,
        path: impl Into<String>,
    ) -> Template<T> {
        // TODO: Don't forget live relaoding in debug mode
        let path = path.into();
        let templates = self.templates.lock().await;
        let template = templates.get(&path);
        if let Some(template) = template {
            template.downcast_ref::<Template<T>>().unwrap().clone()
        } else {
            let template: &str = &std::fs::read_to_string(relative_str(&path))
                .log_err_with_msg("failed to read template")
                .unwrap();

            let mut template_parts = Vec::new();
            let mut start = 0;

            while let Some(open) = template[start..].find("{{") {
                if open > 0 {
                    template_parts.push(Some(&template[start..start + open]));
                }
                start += open;
                if let Some(close) = template[start..].find("}}") {
                    template_parts.push(None);
                    start += close + 2;
                } else {
                    break;
                }
            }
            if start < template.len() {
                template_parts.push(Some(&template[start..]));
            }

            let template_parts = template_parts
                .into_iter()
                .map(|x| x.map(|x| x.to_string()))
                .collect::<Vec<_>>();

            Template::new(template_parts, path)
        }
    }
}

// derive macro didn't wanna work here?
pub struct Template<T> {
    source: String,
    template: Vec<Option<String>>,
    inserts: Vec<(String, T)>,
    phantom: std::marker::PhantomData<T>,
}

impl<T> Clone for Template<T>
where
    T: Into<isize> + Clone,
{
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            template: self.template.clone(),
            inserts: self.inserts.clone(),
            phantom: self.phantom,
        }
    }
}

impl<T> Template<T>
where
    T: Into<isize> + Clone + fmt::Debug,
{
    fn new(template: Vec<Option<String>>, source: String) -> Self {
        Self {
            source,
            template,
            inserts: Vec::new(),
            phantom: std::marker::PhantomData,
        }
    }

    #[must_use]
    pub fn render_only_with(&self, inserts: &[(impl IntoContent, T)]) -> String {
        let mut out = String::new();

        let mut current = 0;
        for part in &self.template {
            if let Some(part) = part {
                out.push_str(part);
            } else {
                let find = inserts.iter().filter(|x| x.1.clone().into() == current);
                for el in find {
                    out.push_str(&el.0.clone().into_content())
                }
                current += 1;
            }
        }

        out
    }

    #[must_use]
    pub fn render(&mut self) -> String {
        let out = self.render_only_with(self.inserts.as_slice());
        self.inserts.clear();
        out
    }

    pub fn insert(&mut self, inserts: &[(impl IntoContent, T)]) {
        self.inserts.extend(
            inserts
                .iter()
                .map(|x| (x.0.clone().into_content(), x.1.clone())),
        );
    }
}

pub trait IntoContent: std::fmt::Debug + Clone {
    fn into_content(self) -> String;
}

impl IntoContent for String {
    fn into_content(self) -> String {
        self
    }
}

impl IntoContent for &str {
    fn into_content(self) -> String {
        self.to_string()
    }
}

impl<T: IntoContent> IntoContent for &[T] {
    fn into_content(self) -> String {
        self.iter()
            .map(|x| x.clone().into_content())
            .collect::<Vec<_>>()
            .join("")
    }
}
