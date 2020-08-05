use crate::config::{Choice, ConfigBool, ConfigInteger, ConfigItem, ConfigSelection, ConfigValue};
use fluent_templates::{static_loader, FluentLoader};
use lazy_static::lazy_static;
use serde::Serialize;
use tera::Tera;

pub trait Page {
    const TEMPLATE_NAME: &'static str;
    fn mock() -> Self;
}

lazy_static! {
    pub static ref TERA: Tera = {
        let mut tera = Tera::new("templates/**/*.html").unwrap();
        tera.register_function(
            "fluent",
            FluentLoader::new(LOCALES.deref()).with_default_lang("en".parse().unwrap()),
        );
        tera
    };
}

static_loader! {
    // Declare our `StaticLoader` named `LOCALES`.
    static LOCALES = {
        // The directory of localizations and fluent resources.
        locales: "./locales",
        // The language to fallback on if something is not present.
        fallback_language: "en-US",
        // Optional: A fluent resource that is shared with every locale.
        // core_locales: "./locales/core.ftl",
    };
}

#[cfg(test)]
fn mock_render<T: Serialize + Page>(data: T) {
    use tera::Context;
    let ctx = Context::from_serialize(data).unwrap();
    TERA.render(T::TEMPLATE_NAME, &ctx).unwrap();
}

macro_rules! test_page {
    ($name:ident) => {
        #[cfg(test)]
        paste::paste! {
            #[test]
            fn [<test_$name:snake>]() {
                mock_render($name::mock())
            }
        }
    };
}


#[derive(Serialize)]
pub struct IndexPage<'a> {
    pub error: Option<&'a str>,
}

impl<'a> Page for IndexPage<'a> {
    const TEMPLATE_NAME: &'static str = "pages/index.html";
    fn mock() -> Self {
        Self {
            error: Some("invalid-key"),
        }
    }
}
test_page!(IndexPage);

#[derive(Serialize)]
pub struct SettingsPage {
    config: Vec<ConfigItem>,
}
impl Page for SettingsPage {
    const TEMPLATE_NAME: &'static str = "pages/settings.html";
    fn mock() -> Self {
        Self {
            config: vec![
                ConfigItem {
                    name: "a".into(),
                    title: "Test A".into(),
                    value: ConfigValue::String("qwerty".into()),
                },
                ConfigItem {
                    name: "b".into(),
                    title: "Test B".into(),
                    value: ConfigValue::Integer(ConfigInteger::new(0, 100, 33).unwrap()),
                },
                ConfigItem {
                    name: "c".into(),
                    title: "Test C".into(),
                    value: ConfigValue::Selection(
                        ConfigSelection::new(
                            "foo".into(),
                            vec![
                                Choice::new("foo".into(), "Use Foo".into()),
                                Choice::new("bar".into(), "Use Bar".into()),
                            ],
                        )
                        .unwrap(),
                    ),
                },
                ConfigItem {
                    name: "d".into(),
                    title: "Test D".into(),
                    value: ConfigValue::Bool(ConfigBool::new(true)),
                },
            ],
        }
    }
}
test_page!(SettingsPage);

#[derive(Serialize)]
pub struct SubmittedPage {}
impl Page for SubmittedPage {
    const TEMPLATE_NAME: &'static str = "pages/submitted.html";
    fn mock() -> Self {
        Self {}
    }
}
test_page!(SubmittedPage);

#[derive(Serialize)]
pub struct ErrorPage<'a> {
    pub message: &'a str,
}
impl<'a> Page for ErrorPage<'a> {
    const TEMPLATE_NAME: &'static str = "pages/error.html";
    fn mock() -> Self {
        Self {
            message: "Dummy error message",
        }
    }
}
test_page!(ErrorPage);
