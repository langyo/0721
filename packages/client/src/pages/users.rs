use stylist::css;
use yew::prelude::*;

#[function_component]
pub fn Users() -> HtmlResult {
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
                {"Images"}
            </h1>
        </div>
    })
}
