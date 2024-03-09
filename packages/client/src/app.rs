use serde::{Deserialize, Serialize};

use hikari_boot::{DeclType, DeriveApplication, DeriveRoutes, RoutesOutsideProps};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::{Footer, Header},
    pages::*,
    utils::global_state::Provider,
};
use _database::types::response::AuthInfo;

#[derive(PartialEq, Clone, Debug, DeriveRoutes, Routable)]
pub enum Routes {
    #[at("/")]
    #[component(Portal)]
    Portal,
    #[at("/dashboard")]
    #[component(Dashboard)]
    Dashboard,

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
}

#[derive(Clone, Debug, DeriveApplication)]
pub struct App;

impl DeclType for App {
    type Routes = Routes;
    type AppStates = AppStates;

    #[allow(non_upper_case_globals)]
    fn decl_render_outside(props: &RoutesOutsideProps<Self::AppStates>) -> yew::Html {
        let theme_raw = r#"
            * {
                margin: 0;
                padding: 0;
                box-sizing: border-box;
                transition: all 0.3s;
                font-family: ENG-CONTENT, CHN-CONTENT;
                color: var(--color-light);
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

        yew::html! {
            <Provider>
                <style>
                    {theme_raw}
                </style>

                <Header />

                <div class={stylist::css!("
                    position: relative;
                    min-height: calc(100vh - 96px - 192px);
                    width: 100%;
                    margin-top: 96px;
                    padding: 64px 0;

                    display: flex;
                    flex-direction: column;
                    align-items: center;
                    justify-content: center;
                ")}>
                    {props.children.clone()}
                </div>

                <Footer />
            </Provider>
        }
    }

    fn render_to_string_outside(
        style_raw: String,
        html_raw: String,
        state: Self::AppStates,
    ) -> String {
        let title = state.title.clone();
        let state = ::serde_json::to_string(&state).unwrap();

        format!("
            <!DOCTYPE html>
            <html>
                <head>
                    <meta charset='utf-8'>
                    <meta name='viewport' content='width=device-width, initial-scale=1'>
                    {style_raw}
                    <title>{title} - Ciallo～(∠·ω< )⌒★</title>
                </head>
                <body>
                    <textarea id='__ssr_data' style='display: none;'>{state}</textarea>
                    <div id='app'>{html_raw}</div>
                    <script src='/client.js'></script>
                    <script>(async () => {{await wasm_vendor_entry('/client.wasm');(await (new wasm_vendor_entry.WebHandle())).start();}})()</script>
                </body>
            </html>
        ")
    }
}
