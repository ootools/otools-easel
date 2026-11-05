mod support;

use std::path::Path;
use std::time::{Duration, Instant};
use otools_easel_native::process::capture;
use support::TestWorkspace;

#[test]
#[cfg(unix)]
fn a_finished_process_does_not_wait_forever_for_background_children_holding_its_pipes() {
    let workspace = TestWorkspace::new("orphan-pipes");
    let start = Instant::now();
    let output = capture(Path::new("/bin/sh"), &["-c".into(), "sleep 4 & printf 'finished\\n'; exit 0".into()], &workspace.root, Duration::from_millis(500), None).unwrap();
    assert!(output.success);
    assert_eq!(output.stdout, "finished\n");
    assert!(start.elapsed() < Duration::from_secs(2), "不应等待继承 stdout 的后台子进程");
}

#[test]
#[cfg(unix)]
fn controlled_process_drains_both_pipes_before_waiting_and_enforces_deadline() {
    let workspace = TestWorkspace::new("process");
    let output = capture(Path::new("/bin/sh"), &["-c".into(), "i=0; while [ $i -lt 4000 ]; do printf 'stdout-data\\n'; printf 'stderr-data\\n' >&2; i=$((i+1)); done".into()], &workspace.root, Duration::from_secs(5), None).unwrap();
    assert!(output.success);
    assert_eq!(output.stdout.lines().count(), 4000);
    assert_eq!(output.stderr.lines().count(), 4000);
    let start = Instant::now();
    let failure = capture(Path::new("/bin/sh"), &["-c".into(), "sleep 30".into()], &workspace.root, Duration::from_millis(100), None).unwrap_err();
    assert!(failure.contains("超时"));
    assert!(start.elapsed() < Duration::from_secs(3));
}
