//! DVT - twisted view. Port of Brian Davis's DVT.LSP (Tonkin & Taylor, 2002).
//!
//! Pick two points for a line at the desired horizontal. The line is drawn on the `defpoints`
//! layer, the UCS is set to World and then to the line (origin at the first point, X axis along
//! the line), the view is turned to plan of that UCS, the snap angle goes to 0 and the current
//! layer back to `0`, and the UCS is saved as `CURRENT_VIEW`.

use ocs_plugin_api::host::{
    CommandStep, HostApi, HostSettingValue, InteractiveCommand, LayerConfig, PreviewWire,
};

const LAYER: &str = "defpoints";
/// The lisp saves the UCS as "CURRENT VIEW" (c:UVS); the host's one-line UCS SAVE cannot take a
/// name with a space, so it is saved as CURRENT_VIEW.
const UCS_NAME: &str = "CURRENT_VIEW";

pub fn start(host: &mut dyn HostApi) {
    // The lisp makes the layer (-LAYER M) and makes it current; the host refuses an existing
    // layer, which is fine.
    let _ = host.add_layer(LayerConfig {
        name: LAYER.to_string(),
        color: None,
        linetype: None,
        lineweight: None,
        off: None,
        frozen: None,
        locked: None,
        plottable: Some(false),
        transparency: None,
        description: None,
    });
    if let Err(e) = host.set_system_variable("CLAYER", HostSettingValue::Text(LAYER.to_string())) {
        host.push_error(&format!("DVT: could not make layer {LAYER} current: {e}"));
    }
    let follow = match host.system_variable("UCSFOLLOW") {
        Some(HostSettingValue::Number(n)) => n != 0.0,
        Some(HostSettingValue::Text(t)) => t.trim() == "1",
        None => false,
    };
    host.start_interactive(Box::new(Dvt { first: None, follow }));
}

struct Dvt {
    first: Option<[f64; 3]>,
    /// UCSFOLLOW as it was, put back after the one-off plan view.
    follow: bool,
}

impl InteractiveCommand for Dvt {
    fn prompt(&self) -> String {
        match self.first {
            None => "DVT  draw a line at the desired horizontal - first point:".to_string(),
            Some(_) => "DVT  second point:".to_string(),
        }
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        let Some(p1) = self.first else {
            self.first = Some(pt);
            return CommandStep::NeedPoint;
        };
        match script(p1, pt, self.follow) {
            Some(s) => CommandStep::Dispatch(s),
            None => CommandStep::Cancel,
        }
    }

    fn on_cursor_move(&mut self, pt: [f64; 3]) -> Vec<PreviewWire> {
        match self.first {
            Some(p1) => vec![PreviewWire::line(p1, pt, None)],
            None => Vec::new(),
        }
    }
}

/// The host command lines that follow the two picks. `None` for a zero-length line.
pub fn script(p1: [f64; 3], p2: [f64; 3], follow: bool) -> Option<String> {
    let d = [p2[0] - p1[0], p2[1] - p1[1], p2[2] - p1[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    // Point on the positive Y side of the new UCS: world Z x line direction, so the new
    // XY plane stays level whatever the line's slope.
    let (hx, hy) = (-d[1], d[0]);
    let hl = (hx * hx + hy * hy).sqrt();
    let yp = if hl < 1e-12 {
        // A vertical line has no horizontal: use world Y.
        [p1[0], p1[1] + 1.0, p1[2]]
    } else {
        [p1[0] + hx / hl, p1[1] + hy / hl, p1[2]]
    };
    let f = |p: [f64; 3]| format!("{},{},{}", p[0], p[1], p[2]);
    Some(format!(
        "UCS WORLD\nLINE {} {}\n\nUCS 3POINT {} {} {}\nUCSFOLLOW 1\nUCSFOLLOW {}\nSETVAR SNAPANG 0\nSETVAR CLAYER 0\nUCS SAVE {}\n",
        f(p1),
        f(p2),
        f(p1),
        f(p2),
        f(yp),
        u8::from(follow),
        UCS_NAME,
    ))
}

#[cfg(test)]
mod tests {
    use super::script;

    #[test]
    fn horizontal_line_east() {
        let s = script([10.0, 5.0, 0.0], [20.0, 5.0, 0.0], false).unwrap();
        assert!(s.starts_with("UCS WORLD\nLINE 10,5,0 20,5,0\n\n"));
        assert!(s.contains("UCS 3POINT 10,5,0 20,5,0 10,6,0\n"));
        assert!(s.ends_with("SETVAR CLAYER 0\nUCS SAVE CURRENT_VIEW\n"));
    }

    #[test]
    fn line_at_45_degrees() {
        let s = script([0.0, 0.0, 0.0], [1.0, 1.0, 0.0], false).unwrap();
        let l = s.lines().find(|l| l.starts_with("UCS 3POINT")).unwrap();
        let yp: Vec<f64> = l.rsplit(' ').next().unwrap().split(',').map(|v| v.parse().unwrap()).collect();
        let r = std::f64::consts::FRAC_1_SQRT_2;
        assert!((yp[0] + r).abs() < 1e-12 && (yp[1] - r).abs() < 1e-12 && yp[2] == 0.0);
    }

    #[test]
    fn zero_length_and_follow_restore() {
        assert!(script([1.0, 1.0, 0.0], [1.0, 1.0, 0.0], false).is_none());
        let s = script([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], true).unwrap();
        assert!(s.contains("UCSFOLLOW 1\nUCSFOLLOW 1\n"));
    }
}
