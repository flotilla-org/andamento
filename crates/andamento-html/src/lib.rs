//! HTML proof of native geometry over the semantic snapshot. This module
//! neither parses templates nor knows about Zellij or terminal cells.
use andamento_core::{
    presentation::{Content, PlacementNode},
    sidebar::{Action, Snapshot},
};

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn action_attribute(action: &Action) -> String {
    escape(&serde_json::to_string(action).expect("semantic actions serialize"))
}

fn content(content: &Content, fallback: &str) -> String {
    if content.fields.is_empty() {
        return escape(fallback);
    }
    content
        .fields
        .iter()
        .map(|field| format!("<span>{}</span>", escape(&field.value)))
        .collect()
}

fn node(node: &PlacementNode) -> String {
    let key = escape(&serde_json::to_string(&node.key).expect("placement keys serialize"));
    let mut result = format!(
        "<article data-placement=\"{key}\" data-layout=\"{}\">",
        escape(node.layout.as_deref().unwrap_or("lines"))
    );
    let activation = action_attribute(&Action::Activate {
        entity: node.entity.clone(),
    });
    if !node.children.is_empty() {
        result.push_str(&format!(
            "<details {} data-toggle=\"{}\"><summary>",
            if node.collapsed { "" } else { "open" },
            action_attribute(&Action::TogglePlacement {
                key: node.key.clone()
            })
        ));
    }
    result.push_str(&format!(
        "<button data-action=\"{activation}\">{}</button>",
        content(&node.content, &node.label)
    ));
    result.push_str(&format!(
        "<small>{}</small>",
        escape(&format!("{:?}", node.state))
    ));
    if !node.children.is_empty() {
        result.push_str("</summary>");
    }
    if let Some(variables) = &node.variables {
        for declaration in &variables.declarations {
            result.push_str(&format!("<label>{}<select data-variable=\"{}\" data-key=\"{key}\"><option value=\"\">Inherit</option>",escape(&declaration.name),escape(&declaration.name)));
            for value in &declaration.values {
                result.push_str(&format!(
                    "<option value=\"{}\" {}>{}</option>",
                    escape(value),
                    if variables
                        .values
                        .get(&declaration.name)
                        .is_some_and(|v| v.value == *value)
                    {
                        "selected"
                    } else {
                        ""
                    },
                    escape(value)
                ));
            }
            result.push_str("</select></label>");
        }
    }
    result.push_str("<div class=\"children\">");
    for child in &node.children {
        result.push_str(&self::node(child));
    }
    result.push_str("</div>");
    if !node.children.is_empty() {
        result.push_str("</details>");
    }
    result.push_str("</article>");
    result
}

pub fn render(snapshot: &Snapshot) -> String {
    let mut html = String::from(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Andamento semantic renderer proof</title><style>
body{font:16px system-ui;margin:2rem;max-width:72rem;color:#20242b;background:#f5f6f8}main{display:flex;gap:2rem;align-items:flex-start;flex-wrap:wrap}section{background:white;border:1px solid #ccd1d9;border-radius:.5rem;padding:1rem;min-width:16rem;flex:1}article{padding:.5rem}.children{margin-left:1rem}article[data-layout=inline]{display:inline-block;vertical-align:top;border:1px solid #ddd;border-radius:.4rem}button,select{font:inherit;padding:.35rem;margin:.2rem}button{background:#f5f7fa;border:1px solid #bcc5d1;border-radius:.3rem;cursor:pointer}button span+span{margin-left:.5em}small{display:block;color:#586575}label{display:inline-block;font-size:.8rem}pre{white-space:pre-wrap;overflow-wrap:anywhere}summary{cursor:pointer}footer{margin-top:2rem}
</style><h1>Andamento</h1><p>Semantic snapshot with native HTML geometry. Controls emit actions; a host owns dispatch and supplies the next snapshot.</p><main>"#,
    );
    for section in &snapshot.surface.sections {
        html.push_str(&format!(
            "<section><h2>{}</h2>",
            content(&section.content, &section.name)
        ));
        for node in &section.nodes {
            html.push_str(&self::node(node));
        }
        for control in &section.content.controls {
            if let Some(name) = &control.variable {
                html.push_str(&format!(
                    "<button data-action=\"{}\">{}: {}</button>",
                    action_attribute(&Action::ToggleDisplayVariable { name: name.clone() }),
                    escape(name),
                    escape(&format!("{:?}", snapshot.surface.display_values.get(name)))
                ));
            }
        }
        html.push_str("</section>");
    }
    html.push_str("</main>");
    for diagnostic in &snapshot.surface.diagnostics {
        html.push_str(&format!("<p>{}</p>", escape(diagnostic)));
    }
    for error in &snapshot.errors {
        html.push_str(&format!(
            "<p>{}: {}</p>",
            escape(&error.entity.id),
            escape(&error.message)
        ));
    }
    html.push_str(r#"<footer><h2>Emitted action</h2><pre id="action">No action yet.</pre></footer><script>
function emit(action){document.getElementById('action').textContent=JSON.stringify(action,null,2);window.dispatchEvent(new CustomEvent('andamento-action',{detail:action}));}
document.querySelectorAll('[data-action]').forEach(el=>el.addEventListener('click',event=>{event.preventDefault();event.stopPropagation();emit(JSON.parse(el.dataset.action));}));
document.querySelectorAll('[data-toggle]').forEach(el=>el.querySelector('summary').addEventListener('click',event=>{if(event.target.closest('button'))return;emit(JSON.parse(el.dataset.toggle));}));
// Keep this payload in sync with Rust sidebar::Action::SetVariable's serde shape.
document.querySelectorAll('[data-variable]').forEach(el=>el.addEventListener('change',()=>emit({action:'set-variable',key:JSON.parse(el.dataset.key),name:el.dataset.variable,value:el.value||null})));
</script></html>"#);
    html
}
