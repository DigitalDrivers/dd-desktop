//! The beacon markers of the `.ldx` the game writes next to each MoTeC file, the engineer's third source of lap
//! boundaries after the app's lap list and the game's log: one marker per crossing of the timing line, its time in
//! microseconds from the file's first sample. On Joe's 17 files of 2026-10-15 every marker matches a
//! `evOnLapCompleted` line of the game's log to the millisecond; on Touristenfahrten that line is at the pit exit,
//! so marker to marker is the whole loop with the parking lot, and the timed lap inside it is found on the corner
//! map ([`super::evaluate`]). The markers stay with the file when the game's log is gone.

/// The markers' times in seconds from the file's start, in order.
pub fn markers(ldx: &str) -> Vec<f64> {
    let mut out: Vec<f64> = ldx
        .split("<Marker ")
        .skip(1)
        .filter(|m| m.contains("ClassName=\"BCN\""))
        .filter_map(|m| m.split("Time=\"").nth(1)?.split('"').next()?.trim().parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0)
        .map(|us| us / 1e6)
        .collect();
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_markers_of_joe_s_file() {
        let ldx = r#"<?xml version="1.0"?><LDXFile Locale="English_Australia.1252" DefaultLocale="C" Version="1.6"><Layers><Layer LayerName=""><MarkerBlock><MarkerGroup Name="Beacons"><Marker Version="100" ClassName="BCN" Name="0, id=99" Flags="13" Time="7.5867e+07" /><Marker Version="100" ClassName="BCN" Name="1, id=99" Flags="13" Time="1.22847e+08" /><Marker Version="100" ClassName="OTHER" Name="x" Time="5" /></MarkerGroup></MarkerBlock></Layer></Layers></LDXFile>"#;
        assert_eq!(markers(ldx), [75.867, 122.847]);
        assert!(markers("").is_empty() && markers("<Marker ClassName=\"BCN\" Time=\"x\" />").is_empty());
    }
}
