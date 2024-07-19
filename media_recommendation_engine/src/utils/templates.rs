use std::fmt::Display;

use askama::Template;

use crate::routes::Section;

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
#[template(path = "../frontend/content/debug_error.html")]
pub struct DebugError<'a> {
    pub err: &'a str,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/settings.html")]
pub struct Settings {
    pub enabled_button: Section,
    pub load_profile: String,
    pub load_admin: Option<String>,
    pub load_account: String,
    pub redirect_back: String,
    pub default_route: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/admin_section.html")]
pub struct AdminSettings {
    pub admin_settings: Vec<Setting>,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/account_section.html")]
pub struct AccountSettings {
    pub account_settings: Vec<Setting>,
}

#[derive(Template)]
#[template(path = "../frontend/content/settings/profile_section.html")]
pub struct ProfileSettings {
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
pub enum CreationInput {
    Text {
        typ: &'static str,
        name: &'static str,
        placeholder: &'static str,
    },
    Checkbox {
        label: &'static str,
        name: &'static str,
        value: &'static str,
    },
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
    pub checked: bool,
    pub location_id: u64,
    pub path: String,
}

impl AsDisplay for LocationEntry {
    fn to_box(self) -> Box<dyn Display> {
        Box::new(self)
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/library/library.html")]
pub struct Library {
    pub load_next: LoadNext,
}

#[derive(Template)]
#[template(path = "../frontend/content/library/load_next.html")]
pub struct LoadNext {
    pub route: String,
    pub page: u64,
    pub per_page: u64,
    random: u32,
}

impl LoadNext {
    pub fn new(route: String, page: u64, per_page: u64) -> Self {
        Self {
            route,
            page,
            per_page,
            random: super::pseudo_random(),
        }
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/library/pagination_response.html")]
pub struct PaginationResponse<T: Template> {
    pub elements: Vec<T>,
    pub load_next: Option<LoadNext>,
}

#[derive(Template)]
#[template(path = "../frontend/content/explore.html")]
pub struct ExploreTemplate;

#[derive(Template)]
#[template(path = "../frontend/content/library/preview.html")]
pub struct PreviewTemplate<'a> {
    pub top: LargeImage,
    pub categories: Vec<(&'a str, LoadNext)>,
}

#[derive(Template)]
#[template(path = "../frontend/content/library/large_preview_image.html")]
pub struct LargeImage {
    pub title: String,
    pub image_interaction: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/library/grid_element.html")]
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
