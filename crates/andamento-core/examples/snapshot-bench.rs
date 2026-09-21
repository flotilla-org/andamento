// Synthetic catalog scaling probe. Run with a host grouping/template configuration.
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
};
use std::{collections::BTreeMap, hint::black_box, time::Instant};
fn main() {
    let config_path = std::env::args()
        .nth(1)
        .expect("usage: snapshot-bench CONFIG.kdl");
    let config = std::fs::read_to_string(config_path).unwrap();
    for n in [25, 50, 100, 200] {
        let mut sidebar = Sidebar::new(&config).unwrap();
        let patches = (0..n)
            .map(|i| MetadataPatch {
                target: MetadataTarget::Entity(EntityRef {
                    kind: "vessel".into(),
                    id: format!("v{i}"),
                }),
                source_id: "bench".into(),
                unset: vec![],
                set: [
                    ("flotilla.project", "p".to_string()),
                    ("flotilla.convoy", format!("c{i}")),
                    ("flotilla.vessel", format!("v{i}")),
                    ("display.label", format!("Vessel {i}")),
                    ("action.primary.recipe", "true".into()),
                ]
                .into_iter()
                .map(|(k, v)| {
                    (
                        k.into(),
                        MetadataValueUpdate {
                            value: MetadataValue::Text(v),
                            ttl_ms: None,
                            precedence: None,
                            ordinal: Some(i),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
            })
            .collect::<Vec<_>>();
        sidebar.apply(100, patches);
        black_box(sidebar.snapshot());
        let t = Instant::now();
        for _ in 0..3 {
            black_box(sidebar.snapshot());
        }
        println!(
            "entities={n} snapshot_ms={:.3}",
            t.elapsed().as_secs_f64() * 1000. / 3.
        );
    }
}
