use stylist::{css, yew::styled_component};
use yew::prelude::*;

use _database::types::config::{load_config, Config};

#[styled_component]
pub fn Footer() -> HtmlResult {
    let global_config = use_prepared_state!((), async move |_| -> Option<Config> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let footer_banner = (*global_config)
        .clone()
        .map(|config| config.portal.footer_banner)
        .unwrap_or_default();

    Ok(html! {
        <footer class={css!("
            position: fixed;
            width: 100%;
            min-height: 64px;
            left: 0;
            bottom: 0;

            background: var(--color-background-header);
            box-shadow: var(--shadow-half);

            display: flex;
            justify-content: center;
            align-items: center;
        ")}>
            <aside class={css!("
                width: 40%;
                margin: 16px 0;

                display: flex;
                flex-direction: column;

                @media (max-width: 991px) {
                    width: 100%;
                }
            ")}>
                {footer_banner.iter().map(|item| {
                    let text = item.text.clone();
                    let href = item.url.clone();

                    let class = css!("
                        width: 100%;
                        line-height: 24px;

                        color: rgba(255, 255, 255, 0.8);
                        font-size: 14px;
                        font-family: ENG-CONTENT, CHN-CONTENT;
                        text-decoration: none;
                        text-align: center;

                        @media (max-width: 991px) {
                            text-align: center;
                        }
                    ");

                    if let Some(href) = href {
                        html! {
                            <a href={href} target={"_blank"} class={class}>
                                {text}
                            </a>
                        }
                    } else {
                        html! {
                            <p class={class}>{text}</p>
                        }
                    }
                }).collect::<Html>()}
            </aside>
        </footer>
    })
}
