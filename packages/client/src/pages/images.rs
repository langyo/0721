use chrono::{FixedOffset, NaiveDate};

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{
    components::icons,
    functions::models::media::{delete, list},
    utils::copy_to_clipboard,
};
use _database::{
    models::media::Model,
    types::config::{load_config, Config},
};

#[styled_component]
pub fn Images() -> HtmlResult {
    let global_config = use_prepared_state!((), async move |_| -> Option<Config> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let media_entry_path = (*global_config)
        .clone()
        .map(|config| config.router.media_entry_path)
        .unwrap_or("/media".to_string());

    let is_downloading = use_state(|| true);
    let image_list = use_state(Vec::new);
    use_effect_with((), {
        let is_downloading = is_downloading.clone();
        let image_list = image_list.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let image_list = image_list.clone();
                // TODO - Pagination.
                let ret = list(Some(0), Some(100)).await.unwrap();
                image_list.set(ret);

                is_downloading.set(false);
            });
        }
    });

    let offset = (*global_config)
        .clone()
        .map(|config| config.portal.timezone)
        .unwrap_or(0);
    let image_list: Vec<(NaiveDate, Vec<Model>)> =
        image_list.iter().fold(Vec::new(), |mut acc, item| {
            let date = item.created_at.clone();
            // Parse UTC date to local date with time zone what be defined in the global config.
            let date = date.with_timezone(&FixedOffset::east_opt(offset * 3600).unwrap());
            let date = date.date_naive();

            if let Some(last) = acc.last_mut() {
                if last.0 == date {
                    last.1.push(item.clone());
                } else {
                    acc.push((date, vec![item.clone()]));
                }
            } else {
                acc.push((date, vec![item.clone()]));
            }
            acc
        });

    Ok(html! {
        <>
            {
                if *is_downloading {
                    html! {
                        <div class={css!("
                            position: fixed;
                            width: 100vw;
                            height: 100vh;
                            left: 0;
                            top: 0;

                            display: flex;
                            flex-direction: column;
                            align-items: center;
                            justify-content: center;
                        ")}>
                            <icons::CircularProgress />
                        </div>
                    }
                } else {
                    html! {
                        <div class={css!("
                            position: relative;
                            width: 100%;
                            margin-top: 96px;
                            margin-bottom: 64px;
                            padding: 64px;

                            display: flex;
                            flex-direction: column;
                            align-items: flex-start;
                            justify-content: flex-start;
                        ")}>
                            {
                                image_list.iter().map(|items| html! {
                                    <>
                                        <span class={css!("
                                            width: 100%;
                                            margin: 16px 8px;

                                            font-size: 24px;
                                            font-weight: bolder;
                                            user-select: none;
                                        ")}>
                                            {items.0.format("%Y-%m-%d").to_string()}
                                        </span>

                                        <div class={css!("
                                            width: 100%;
                                            margin: 32px 0;

                                            display: flex;
                                            flex-wrap: wrap;
                                            align-items: flex-start;
                                            justify-content: flex-start;
                                        ")}>
                                            {
                                                items.1.iter().map(|item| html! {
                                                    <div
                                                        class={css!("
                                                            position: relative;
                                                            width: 128px;
                                                            height: 128px;
                                                            margin: 8px;

                                                            border-radius: 4px;
                                                            box-shadow: var(--shadow-half);

                                                            display: flex;
                                                            flex-wrap: wrap;
                                                            align-items: center;
                                                            justify-content: center;

                                                            --image-filter: none;
                                                            --show: 0;

                                                            &:hover {
                                                                --image-filter: brightness(0.2);
                                                                --show: 1;
                                                            }
                                                        ")}
                                                    >
                                                        <img
                                                            class={css!("
                                                                position: absolute;
                                                                top: 0;
                                                                left: 0;
                                                                width: 100%;
                                                                height: 100%;

                                                                object-fit: cover;
                                                                border-radius: 4px;
                                                                filter: var(--image-filter);
                                                                user-select: none;
                                                            ")}
                                                            src={format!("{}/{}?thumbnail=true", media_entry_path, item.hash)}
                                                        />

                                                        <span class={css!("
                                                            width: 128px;
                                                            height: 64px;

                                                            display: flex;
                                                            flex-direction: column;
                                                            align-items: center;
                                                            justify-content: center;

                                                            opacity: var(--show);
                                                            z-index: 1;
                                                        ")}>
                                                            <p class={css!("
                                                                line-height: 24px;
                                                                font-size: 16px;
                                                                font-weight: bolder;
                                                            ")}>
                                                                {item.uploader.clone()}
                                                            </p>
                                                            <p class={css!("
                                                                line-height: 16px;
                                                                font-size: 12px;
                                                                user-select: none;
                                                            ")}>
                                                                {item.mime.clone()}
                                                            </p>
                                                        </span>

                                                        <div
                                                            class={css!("
                                                                width: 64px;
                                                                height: 64px;
                                                                border-radius: 0 0 0 4px;

                                                                display: flex;
                                                                align-items: center;
                                                                justify-content: center;

                                                                user-select: none;
                                                                cursor: pointer;
                                                                opacity: var(--show);
                                                                z-index: 1;

                                                                &:hover {
                                                                    background: var(--color-light-less);
                                                                }

                                                                &:active {
                                                                    background: var(--color-light-most);
                                                                }
                                                            ")}
                                                            onclick={
                                                                let origin = gloo::utils::document().location().unwrap().origin().unwrap();
                                                                let media_entry_path = media_entry_path.clone();
                                                                let hash = item.hash.clone();
                                                                Callback::from(move |_| {
                                                                    copy_to_clipboard(format!("{}{}/{}", origin, media_entry_path, hash));
                                                                    gloo::dialogs::alert("Copied to clipboard!");
                                                                })
                                                            }
                                                        >
                                                            <icons::Copy size={24} />
                                                        </div>

                                                        <div
                                                            class={css!("
                                                                width: 64px;
                                                                height: 64px;
                                                                border-radius: 0 0 4px 0;

                                                                display: flex;
                                                                align-items: center;
                                                                justify-content: center;

                                                                user-select: none;
                                                                cursor: pointer;
                                                                opacity: var(--show);
                                                                z-index: 1;

                                                                &:hover {
                                                                    background: var(--color-light-less);
                                                                }

                                                                &:active {
                                                                    background: var(--color-light-most);
                                                                }
                                                            ")}
                                                            onclick={
                                                                let hash = item.hash.clone();
                                                                Callback::from(move |_| {
                                                                    let hash = hash.to_owned();
                                                                    wasm_bindgen_futures::spawn_local(async move {
                                                                        if let Ok(_) = delete(hash.to_owned()).await {
                                                                            gloo::dialogs::alert("Deleted!");
                                                                        } else {
                                                                            gloo::dialogs::alert("Failed to delete!");
                                                                        }
                                                                    });
                                                                })
                                                            }
                                                        >
                                                            <icons::Delete size={24} />
                                                        </div>
                                                    </div>
                                                }).collect::<Html>()
                                            }
                                        </div>
                                    </>
                                }).collect::<Html>()
                            }
                        </div>
                    }
                }
            }
        </>
    })
}
