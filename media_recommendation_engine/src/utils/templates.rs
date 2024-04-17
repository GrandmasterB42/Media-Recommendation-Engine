use askama::Template;

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
    pub message: Option<String>,
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
#[template(path = "../frontend/content/settings.html")]
pub struct Settings<'a> {
    pub admin_settings: Option<Vec<Setting<'a>>>,
    pub account_settings: Vec<Setting<'a>>,
    pub redirect_back: String,
    pub name: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/setting.html")]
pub enum Setting<'a> {
    TextSetting {
        prompt: &'a str,
        action: &'a str,
    },
    Button {
        label: &'a str,
        class: &'a str,
        action: &'a str,
    },
}

#[derive(Template)]
#[template(path = "../frontend/content/library.html")]
pub struct Library {
    pub sessions: Vec<GridElement>,
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

#[derive(Template, Clone)]
#[template(path = "../frontend/content/recommendation_popup.html")]
pub struct RecommendationPopup {
    pub id: u64,
    pub image: String,
    pub title: String,
}

#[derive(Template, Clone)]
#[template(path = "../frontend/content/swap_in.html")]
pub struct SwapIn<'a> {
    pub swap_id: &'a str,
    pub content: &'a str,
}
