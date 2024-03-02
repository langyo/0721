use stylist::{css, yew::styled_component};
use yew::prelude::*;

#[styled_component]
pub fn Footer() -> Html {
    html! {
        <footer class={css!("
            position: absolute;
            width: 100%;
            height: 64px;
            left: 0;
            bottom: 0;

            background: var(--color-background-header);
            padding: 24px;
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
                {vec![
                    // TODO - Custom the href and text by global config
                    (Some("https://github.com/langyo"), "Some banner"),
                ].into_iter().map(|(href, text)| {
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
                            <a href={href} class={class}>
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
    }
}
