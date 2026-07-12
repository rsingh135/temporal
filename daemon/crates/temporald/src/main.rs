use fable_library_rust::Native_::LrcPtr;
use fable_library_rust::String_::string;
use temporal_core::Temporal::Domain::Types::{
    geometryArea, workspaceIdValue, WindowGeometry, WorkspaceId,
};

fn main() {
    let id = LrcPtr::new(WorkspaceId::WorkspaceId(string("m0-smoke")));
    let geometry = LrcPtr::new(WindowGeometry {
        X: 100.0,
        Y: 200.0,
        Width: 1280.0,
        Height: 800.0,
    });
    println!(
        "temporald M0 smoke: workspace={} area={}",
        workspaceIdValue(id),
        geometryArea(geometry)
    );
}
