//! Choose a tile arrangement from the actual workspace, not a screen-width cutoff.
pub(super) fn columns(count: usize, width: f32, height: f32) -> usize {
    (1..=count.clamp(1, 4))
        .min_by(|a, b| {
            let score = |columns: usize| {
                let rows = count.div_ceil(columns).max(1);
                (0..rows)
                    .map(|row| {
                        let cells = columns.min(count.saturating_sub(row * columns)).max(1);
                        let aspect = (width / cells as f32) / (height.max(1.0) / rows as f32);
                        (aspect / (16.0 / 9.0)).ln().powi(2) * cells as f32
                    })
                    .sum::<f32>()
            };
            score(*a).total_cmp(&score(*b))
        })
        .unwrap_or(1)
}
