use stylist::{css, yew::styled_component};
use yew::prelude::*;

#[styled_component]
pub fn NotFound() -> Html {
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
            {
                vec!["404", "Not Found"].into_iter().map(|text| {
                    html! {
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
                            {text}
                        </h1>
                    }
                }).collect::<Html>()
            }
        </div>

    }
}
