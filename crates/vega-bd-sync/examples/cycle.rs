//! One sync cycle over the projects in `$XDG_STATE_HOME/vega/bd`, as vegad
//! runs it; for trying the sync on throwaway projects without a vegad:
//!
//! XDG_STATE_HOME=/tmp/x cargo run -p vega-bd-sync --example cycle

fn main() {
    let mut ops = vega_bd_sync::sync::Bd {
        actor: "example/vegad".into(),
    };
    for e in vega_bd_sync::sync::cycle(vega_bd_sync::unix_now(), &mut ops) {
        println!("{e:?}");
    }
}
