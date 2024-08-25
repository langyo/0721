use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::utils::global_state::GlobalStateContext;
use _types::config::{load_config, Config as Model};

#[styled_component]
pub fn ConfigPage() -> HtmlResult {
    let global_config = use_prepared_state!((), async move |_| -> Option<Model> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let global_config = use_state(|| (*global_config).clone().unwrap_or_default());

    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    Ok(html! {
        <div class={css!("
            position: relative;
            width: 100%;
            margin-top: 96px;
            margin-bottom: 64px;
            padding: 16px;

            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
        ")}>
            <div class={css!("
                width: 80%;
                margin: 16px;
                padding: 0 32px;

                display: flex;
                flex-direction: column;
                align-items: center;
                justify-content: center;

                background: var(--color-light-less);
                border-radius: 4px;
                box-shadow: var(--shadow-half);
            ")}>
                {
                    [
                        (
                            t.config.portal.engine_version.clone(),
                            html! {
                                {env!("CARGO_PKG_VERSION")}
                            }
                        ),
                        (
                            t.config.portal.title_suffix.clone(),
                            html! {
                                {global_config.portal.title_suffix.clone()}
                            }
                        ),
                        (
                            t.config.portal.footer_banner.clone(),
                            html! {
                                global_config.portal.footer_banner.iter().map(|item| html! {
                                    <a
                                        class={css!("
                                            width: 100%;
                                            height: 32px;
                                            padding: 0 16px;

                                            font-size: 16px;
                                            line-height: 32px;
                                            text-align: center;
                                            user-select: none;
                                        ")}
                                        href={item.url.clone().unwrap_or_default()}
                                        target="_blank"
                                    >
                                        {item.text.clone()}
                                    </a>
                                }).collect::<Html>()
                            }
                        ),
                        (
                            t.config.portal.language.clone(),
                            html! {
                                {
                                    global_config.portal.language.clone()
                                }
                            },
                        ),
                        (
                            t.config.portal.timezone.clone(),
                            html! {
                                {
                                    global_config.portal.timezone.to_string()
                                }
                            },
                        ),
                        (
                            t.config.router.media_entry_path.clone(),
                            html! {
                                {
                                    global_config.router.media_entry_path.clone()
                                }
                            },
                        ),
                        (
                            t.config.router.limit_referrer_host.clone(),
                            {
                                if let Some(val) = &global_config.router.limit_referrer_host {
                                    if !val.is_empty() {
                                        return Ok(val.iter().map(|item| html! {
                                            <a
                                                class={css!("
                                                    width: 100%;
                                                    height: 32px;
                                                    padding: 0 16px;

                                                    font-size: 16px;
                                                    line-height: 32px;
                                                    text-align: center;
                                                    user-select: none;
                                                ")}
                                                href={item.clone()}
                                                target="_blank"
                                            >
                                                {item.clone()}
                                            </a>
                                        }).collect::<Html>());
                                    }
                                }
                                html! {
                                    <p class={css!("
                                        width: 100%;
                                        height: 32px;

                                        font-size: 16px;
                                        line-height: 32px;
                                        text-align: center;
                                        font-style: italic;
                                        user-select: none;
                                    ")}>
                                        {"None"}
                                    </p>
                                }
                            }
                        ),
                        (
                            t.config.upload.image_size_limit.clone(),
                            html! {
                                {
                                    global_config.upload.image_size_limit.clone()
                                }
                            },
                        ),
                        (
                            t.config.upload.webp_auto_convert.clone(),
                            html! {
                                if global_config.upload.webp_auto_convert {
                                    {"true"}
                                } else {
                                    {"false"}
                                }
                            },
                        ),
                        (
                            t.config.upload.use_source_file_name.clone(),
                            html! {
                                if global_config.upload.use_source_file_name {
                                    {"true"}
                                } else {
                                    {"false"}
                                }
                            },
                        ),
                    ].iter().map(|(name, value)| html! {
                        <div class={css!("
                            width: 100%;
                            min-height: 64px;

                            display: flex;
                            align-items: center;
                            justify-content: space-between;
                        ")}>
                            <span class={css!("
                                width: 20%;
                                min-width: 128px;
                                height: 32px;

                                font-size: 16px;
                                line-height: 32px;
                                user-select: none;
                            ")}>
                                {name.to_string()}
                            </span>
                            <span
                                class={css!("
                                    width: 80%;
                                    min-height: 32px;
                                    padding: 0 16px;

                                    border: none;
                                    outline: 1px solid var(--color-light-half);

                                    font-size: 16px;
                                    line-height: 32px;
                                    user-select: none;

                                    display: flex;
                                    flex-direction: column;
                                    align-items: center;
                                    justify-content: center;

                                    background: var(--color-dark-less);
                                    border-radius: 4px;
                                    box-shadow: var(--shadow-half);
                                ")}
                            >
                                {value.clone()}
                            </span>
                        </div>
                    }).collect::<Html>()
                }
            </div>
        </div>
    })
}
