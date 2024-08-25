use chrono::{FixedOffset, NaiveDate};

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::components::{icons, GlobalSkeleton};
use _functions::{
    models::media::{count, delete, list},
    utils::{copy_to_clipboard, global_state::GlobalStateContext},
};
use _types::{
    config::{load_config, Config},
    models::media::Model,
};

#[styled_component]
pub fn Images() -> HtmlResult {
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

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
    let image_list_ref = use_state(Vec::new);
    use_effect_with((), {
        let is_downloading = is_downloading.clone();
        let image_list_ref = image_list_ref.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let image_list = image_list_ref.clone();
                let ret = list(Some(0), Some(64)).await.unwrap();
                image_list.set(ret);

                is_downloading.set(false);
            });
        }
    });

    let total_count = use_state(|| 0);
    use_effect_with((), {
        let total_count = total_count.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let total_count = total_count.clone();
                let ret = count().await.unwrap();
                total_count.set(ret);
            });
        }
    });

    let offset = (*global_config)
        .clone()
        .map(|config| config.portal.timezone)
        .unwrap_or(0);
    let image_list: Vec<(NaiveDate, Vec<Model>)> =
        image_list_ref.iter().fold(Vec::new(), |mut acc, item| {
            let date = item.created_at;
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
            <GlobalSkeleton is_loading={*is_downloading} />

            <div class={css!("
                position: relative;
                width: 100%;
                margin-top: 96px;
                padding: 64px;

                display: flex;
                flex-direction: column;
                align-items: flex-start;
                justify-content: flex-start;
            ")}>
                <span class={css!("
                    width: 100%;
                    margin: 16px 8px;

                    font-size: 24px;
                    font-weight: bolder;
                    text-align: center;
                    user-select: none;
                ")}>
                    {format!(
                        "{} {}",
                        t.images.total_count,
                        *total_count
                    )}
                </span>

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
                                            key={item.name.clone()}
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
                                                src={format!("{}/{}?thumbnail=true", media_entry_path, item.name)}
                                            />

                                            <span
                                                class={css!("
                                                    width: 128px;
                                                    height: 64px;

                                                    display: flex;
                                                    flex-direction: column;
                                                    align-items: center;
                                                    justify-content: center;

                                                    opacity: var(--show);
                                                    z-index: 1;
                                                    user-select: none;
                                                    cursor: pointer;
                                                ")}
                                                onclick={Callback::from({
                                                    let origin = gloo::utils::document().location().unwrap().origin().unwrap();
                                                    let media_entry_path = media_entry_path.clone();
                                                    let name = item.name.clone();
                                                    move |_| {
                                                        let url = format!("{}{}/{}", origin, media_entry_path, name);
                                                        gloo::utils::window().open_with_url_and_target(&url, "_blank").unwrap();
                                                    }
                                                })}
                                            >
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
                                                    let name = item.name.clone();
                                                    Callback::from(move |_| {
                                                        copy_to_clipboard(format!("{}{}/{}", origin, media_entry_path, name));
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
                                                    let name = item.name.clone();
                                                    Callback::from(move |_| {
                                                        let name = name.to_owned();
                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            if delete(name.to_owned()).await.is_ok() {
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

            <div class={css!("
                width: 100%;
                margin-top: 32px;
                margin-bottom: 128px;

                display: flex;
                align-items: center;
                justify-content: center;
            ")}>
                <button
                    class={classes!(css!("
                        width: 128px;
                        height: 32px;

                        font-size: 16px;
                        font-weight: bolder;
                        text-align: center;
                    "), {
                        if image_list.iter().fold(0, |acc, item| acc + item.1.len()) >= *total_count {
                            css!("
                                background: var(--color-dark-less);
                                pointer-events: none;
                            ")
                        } else {
                            css!("")
                        }
                    })}
                    onclick={
                        let is_downloading = is_downloading.clone();
                        let image_list_ref = image_list_ref.clone();

                        Callback::from(move |_| {
                            let is_downloading = is_downloading.clone();
                            let image_list_ref = image_list_ref.clone();
                            is_downloading.set(true);

                            wasm_bindgen_futures::spawn_local(async move {
                                let image_list = image_list_ref.clone();
                                let ret = list(Some(image_list_ref.iter().len()), Some(64)).await.unwrap();
                                image_list.set(image_list_ref.iter().chain(ret.iter()).cloned().collect::<Vec<Model>>());

                                is_downloading.set(false);
                            });
                        })
                    }
                >
                    { t.images.load_more }
                </button>
            </div>
        </>
    })
}
