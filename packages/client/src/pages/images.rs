use stylist::css;
use yew::prelude::*;

use crate::functions::models::media::list;
use _database::types::config::{load_config, Config};

#[function_component]
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

    let image_list = use_state(|| Vec::new());
    use_effect_with((), {
        let image_list = image_list.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let image_list = image_list.clone();
                let ret = list(Some(0), Some(30)).await.unwrap();
                image_list.set(ret);
            });
        }
    });

    Ok(html! {
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
            <h1 class={css!("
                height: 48px;
                width: 100%;
                margin: 16px;

                line-height: 48px;
                text-align: center;

                font-size: 24px;
                font-weight: bolder;
                user-select: none;
            ")}>
                {
                    image_list.iter().map(|item| html! {
                        <img
                            class={css!("
                                width: 100px;
                                height: 100px;
                                margin: 8px;
                            ")}
                            src={format!("{}/{}?width=100&height=100", media_entry_path, item.hash)}
                        />
                    }).collect::<Html>()
                }
            </h1>
        </div>
    })
}
