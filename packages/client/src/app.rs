use anyhow::Result;
use serde::{Deserialize, Serialize};

use hikari_boot::{DeclType, DeriveApplication, DeriveRoutes, RoutesOutsideProps};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::{Footer, Header},
    pages::*,
    utils::global_state::GlobalStateProvider,
};
use _database::types::{config::Config, i18n::Language, response::AuthInfo};

#[derive(PartialEq, Clone, Debug, DeriveRoutes, Routable)]
pub enum Routes {
    #[at("/")]
    #[component(Portal)]
    Portal,
    #[at("/images")]
    #[component(Images)]
    Images,
    #[at("/users")]
    #[component(Users)]
    Users,
    #[at("/config")]
    #[component(ConfigPage)]
    Config,

    #[at("/login")]
    #[component(Login)]
    Login,
    #[at("/register")]
    #[component(Register)]
    Register,
    #[not_found]
    #[at("/404")]
    #[component(NotFound)]
    NotFound,
}

#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct AppStates {
    pub title: String,
    pub auth: AuthInfo,
    pub language: Language,
    pub config: Config,
}

#[derive(Clone, Debug, DeriveApplication)]
pub struct App;

impl DeclType for App {
    type Routes = Routes;
    type AppStates = AppStates;

    #[allow(non_upper_case_globals)]
    fn decl_render_outside(props: &RoutesOutsideProps<Self::AppStates>) -> yew::HtmlResult {
        let theme_raw = r#"
            * {
                margin: 0;
                padding: 0;
                box-sizing: border-box;
                transition: all 0.3s;
                font-family: ENG-CONTENT, CHN-CONTENT;
            }

            :root {
                --color-background-header: rgba(46, 34, 61, 0.5);
                --color-background-footer: rgba(67, 68, 90, 0.5);

                --color-primary: rgb(150, 98, 150);
                --color-primary-most: rgba(150, 98, 150, 0.8);
                --color-primary-half: rgba(150, 98, 150, 0.5);
                --color-primary-less: rgba(150, 98, 150, 0.2);

                --color-secondary: rgb(176, 163, 170);
                --color-secondary-most: rgba(176, 163, 170, 0.8);
                --color-secondary-half: rgba(176, 163, 170, 0.5);
                --color-secondary-less: rgba(176, 163, 170, 0.2);

                --color-dark: rgb(0, 0, 0);
                --color-dark-most: rgba(0, 0, 0, 0.8);
                --color-dark-half: rgba(0, 0, 0, 0.5);
                --color-dark-less: rgba(0, 0, 0, 0.2);

                --color-light: rgb(255, 255, 255);
                --color-light-most: rgba(255, 255, 255, 0.8);
                --color-light-half: rgba(255, 255, 255, 0.5);
                --color-light-less: rgba(255, 255, 255, 0.2);

                --icon-color: var(--color-light);
                --shadow-half: 0 0 8px rgba(0, 0, 0, 0.5);

                background: var(--color-dark-most);
                overflow-x: hidden;
            }

            h1, h2, h3, h4, h5, h6, p, a, span, div, input, button {
                color: var(--color-light);
            }

            button {
                border: none;
                outline: none;
                border-radius: 4px;
                box-shadow: var(--shadow-half);

                background: var(--color-primary-most);
                color: var(--color-light-most);
                user-select: none;
                cursor: pointer;
            }

            button:hover {
                background: var(--color-primary);
                color: var(--color-light);
            }
        "#;

        Ok(yew::html! {
            <GlobalStateProvider language={props.states.language}>
                <style>
                    {theme_raw}
                </style>

                <Header />

                {props.children.clone()}

                <Footer />
            </GlobalStateProvider>
        })
    }

    fn render_to_string_outside(
        style_raw: String,
        html_raw: String,
        state: Self::AppStates,
    ) -> Result<String> {
        let title = state.title.clone();
        let title_suffix = state.config.portal.title_suffix.clone();

        let state_raw = ::serde_json::to_string(&state).unwrap();

        Ok(format!("
            <!DOCTYPE html>
            <html>
                <head>
                    <meta charset='utf-8'>
                    <meta name='viewport' content='width=device-width, initial-scale=1'>
                    {style_raw}
                    <title>{title} - {title_suffix}</title>
                </head>
                <body>
                    <textarea id='__ssr_data' style='display: none;'>{state_raw}</textarea>
                    <div id='app'>{html_raw}</div>
                    <script src='/client.js'></script>
                    <script>(async () => {{await wasm_vendor_entry('/client.wasm');(await (new wasm_vendor_entry.WebHandle())).start();}})()</script>
                </body>
            </html>
        "))
    }
}
