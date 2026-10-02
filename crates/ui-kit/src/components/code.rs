use leptos::prelude::*;

pub fn line_count(text: &str) -> usize {
    text.split('\n').count().max(1)
}

/// Monospace multi-line editor with line numbers (JVM arguments).
#[component]
pub fn CodeEditor(
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
) -> impl IntoView {
    let gutter = move || (1..=line_count(&value.get())).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
    view! {
        <div class="code">
            <div class="code__gutter">{gutter}</div>
            <textarea class="code__area" spellcheck="false" placeholder=move || placeholder.get() bind:value=value></textarea>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_with_minimum_of_one() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("-Xmx4G"), 1);
        assert_eq!(line_count("a\nb\n"), 3);
    }
}
