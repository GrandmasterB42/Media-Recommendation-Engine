use std::fmt::Display;

use askama::Template;

pub trait AsDisplay: Display {
    fn to_box(self) -> Box<dyn Display>;
}

#[derive(Template)]
#[template(path = "../frontend/content/index.html")]
pub struct Index {
    pub body: String,
    pub all: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/login.html")]
pub struct LoginPage<'a> {
    pub title: &'a str,
    pub post_url: &'a str,
    pub sub_text: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "../frontend/content/homepage.html")]
pub struct Homepage<'a> {
    pub redirect_library: &'a str,
    pub redirect_explore: &'a str,
    pub redirect_settings: &'a str,
    pub content: &'a str,
    pub route: &'a str,
}

#[derive(Template)]
#[template(path = "../frontend/content/error.html")]
pub struct Error<'a> {
    pub err: &'a str,
    pub redirect: &'a str,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/settings.html")]
pub struct Settings {
    pub admin_settings: Option<Vec<Setting>>,
    pub account_settings: Vec<Setting>,
    pub redirect_back: String,
    pub name: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/setting.html")]
pub enum Setting {
    CreationMenu { creation: Creation },
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/creation.html")]
pub struct Creation {
    pub title: &'static str,
    pub list_id: &'static str,
    pub error_id: &'static str,
    pub post_addr: &'static str,
    pub entries: Vec<Box<dyn Display>>,
    pub inputs: Vec<CreationInput>,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/creation_input.html")]
pub struct CreationInput {
    pub typ: &'static str,
    pub name: &'static str,
    pub placeholder: &'static str,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/user_entry.html")]
pub struct UserEntry {
    pub user_id: u64,
    pub name: String,
    pub can_delete: bool,
}

impl AsDisplay for UserEntry {
    fn to_box(self) -> Box<dyn Display> {
        Box::new(self)
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/location_entry.html")]
pub struct LocationEntry {
    pub location_id: u64,
    pub path: String,
}

impl AsDisplay for LocationEntry {
    fn to_box(self) -> Box<dyn Display> {
        Box::new(self)
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/library.html")]
pub struct Library {
    pub franchises: Vec<GridElement>,
}

#[derive(Template)]
#[template(path = "../frontend/content/explore.html")]
pub struct ExploreTemplate;

#[derive(Template)]
#[template(path = "../frontend/content/preview.html")]
pub struct PreviewTemplate<'a> {
    pub top: LargeImage,
    pub categories: Vec<(&'a str, Vec<GridElement>)>,
}

#[derive(Template)]
#[template(path = "../frontend/content/large_preview_image.html")]
pub struct LargeImage {
    pub title: String,
    pub image_interaction: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/grid_element.html")]
pub struct GridElement {
    pub title: String,
    pub redirect_entire: String,
    pub redirect_img: String,
    pub redirect_title: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/video.html")]
pub struct Video {
    pub id: u64,
}

#[derive(Template)]
#[template(path = "../frontend/content/notification.html")]
pub struct Notification<'a> {
    pub msg: String,
    pub script: &'a str,
}

#[derive(Template, Clone)]
#[template(path = "../frontend/content/recommendation_popup.html")]
pub struct RecommendationPopup {
    pub id: u64,
    pub image: String,
    pub title: String,
}

#[derive(Template, Clone)]
#[template(path = "../frontend/content/swap_in.html")]
pub struct SwapIn<'a, T>
where
    T: Display,
{
    pub swap_id: &'a str,
    pub swap_method: Option<&'a str>,
    pub content: T,
}
