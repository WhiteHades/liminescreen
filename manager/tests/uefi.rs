#![forbid(unsafe_code)]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

struct Vm(Child);
impl Drop for Vm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn screenshot(socket: &Path, output: &Path) {
    let mut client = UnixStream::connect(socket).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(client.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    for request in [
        json!({"execute": "qmp_capabilities"}),
        json!({"execute": "screendump", "arguments": {"filename": output, "format": "png"}}),
    ] {
        writeln!(client, "{request}").unwrap();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            let response: Value = serde_json::from_str(&line).unwrap();
            assert!(response.get("error").is_none(), "{response}");
            if response.get("return").is_some() {
                break;
            }
        }
    }
    assert!(fs::metadata(output).unwrap().len() > 1000);
}

fn run_case(
    root: &Path,
    esp: &Path,
    label: &str,
    expected: &str,
    gpus: u8,
    capture: Option<&Path>,
) {
    let code = std::env::var("OVMF_CODE").unwrap_or("/usr/share/edk2/x64/OVMF_CODE.4m.fd".into());
    let template =
        std::env::var("OVMF_VARS").unwrap_or("/usr/share/edk2/x64/OVMF_VARS.4m.fd".into());
    let vars = root.join(format!("{label}.fd"));
    fs::copy(template, &vars).unwrap();
    let log = root.join(format!("{label}.log"));
    let qmp = root.join("qmp");
    let _ = fs::remove_file(&qmp);
    let mut command = Command::new("qemu-system-x86_64");
    command
        .args(["-machine", "q35", "-m", "256", "-net", "none"])
        .args([
            "-drive",
            &format!("if=pflash,format=raw,readonly=on,file={code}"),
        ])
        .args([
            "-drive",
            &format!("if=pflash,format=raw,file={}", vars.display()),
        ])
        .args([
            "-drive",
            &format!("format=raw,file=fat:rw:{}", esp.display()),
        ])
        .args([
            "-display",
            "none",
            "-serial",
            "none",
            "-monitor",
            "none",
            "-no-reboot",
        ])
        .args([
            "-debugcon",
            &format!("file:{}", log.display()),
            "-global",
            "isa-debugcon.iobase=0xe9",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match gpus {
        0 => {
            command.args(["-vga", "none"]);
        }
        2 => {
            command.args(["-device", "secondary-vga"]);
        }
        _ => {}
    }
    if capture.is_some() {
        command.args([
            "-qmp",
            &format!("unix:{},server=on,wait=off", qmp.display()),
        ]);
    }
    let mut vm = Vm(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut text = String::new();
    while Instant::now() < deadline && vm.0.try_wait().unwrap().is_none() {
        text = fs::read_to_string(&log).unwrap_or_default();
        if text.contains(expected) {
            if gpus == 2 {
                assert!(
                    text.lines().any(|line| {
                        line.split("liminescreen: ").nth(1).is_some_and(|message| {
                            message.contains("firmware graphics interfaces")
                                && message
                                    .split_whitespace()
                                    .next()
                                    .unwrap()
                                    .parse::<u32>()
                                    .unwrap()
                                    >= 2
                        })
                    }),
                    "{text}"
                );
            }
            if let Some(output) = capture {
                sleep(Duration::from_secs(3));
                screenshot(&qmp, output);
            }
            println!("pass {label}: {expected}");
            return;
        }
        sleep(Duration::from_millis(100));
    }
    let mut error = String::new();
    if vm.0.try_wait().unwrap().is_some() {
        vm.0.stderr
            .as_mut()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
    }
    panic!("{label}: missing {expected}\n{text}\n{error}");
}

#[test]
#[ignore = "requires qemu and ovmf; runs only on a temporary virtual ESP"]
fn chainload_updates_and_failures() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temporary_root = project.join(".tmp");
    fs::create_dir_all(&temporary_root).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("vm")
        .tempdir_in(&temporary_root)
        .unwrap();
    let root = temporary.path();
    let esp = root.join("esp");
    let boot = esp.join("EFI/BOOT");
    let limine = esp.join("EFI/limine");
    fs::create_dir_all(&boot).unwrap();
    fs::create_dir_all(&limine).unwrap();
    let artifacts = root.join("target/x86_64-unknown-uefi/release");
    let build = |binary: &str, marker: &str| {
        assert!(
            Command::new("cargo")
                .current_dir(project)
                .args([
                    "build",
                    "--locked",
                    "--release",
                    "-p",
                    "liminescreen",
                    "--target",
                    "x86_64-unknown-uefi",
                    "--features",
                    "vm-tests",
                    "--bin",
                    binary
                ])
                .env("CARGO_TARGET_DIR", root.join("target"))
                .env("LIMINESCREEN_TEST_MARKER", marker)
                .status()
                .unwrap()
                .success()
        );
    };
    build("liminescreen", "unused");
    fs::copy(artifacts.join("liminescreen.efi"), boot.join("BOOTX64.EFI")).unwrap();
    for (marker, gpus) in [("FIRST", 1), ("UPDATED", 2)] {
        build("boot-probe", marker);
        fs::copy(
            artifacts.join("boot-probe.efi"),
            limine.join("limine_x64.efi"),
        )
        .unwrap();
        run_case(
            root,
            &esp,
            marker,
            &format!("LIMINESCREEN_TARGET_{marker}"),
            gpus,
            None,
        );
    }
    run_case(
        root,
        &esp,
        "headless",
        "LIMINESCREEN_TARGET_UPDATED",
        0,
        None,
    );
    fs::remove_file(limine.join("limine_x64.efi")).unwrap();
    run_case(
        root,
        &esp,
        "missing",
        "cannot start installed Limine",
        1,
        None,
    );
    if let Ok(official) = std::env::var("LIMINE_EFI") {
        fs::copy(official, limine.join("limine_x64.efi")).unwrap();
        fs::write(esp.join("limine.conf"), "timeout: no\ninterface_resolution: 1024x768\n/Omarchy\nprotocol: efi\npath: boot():/unused-linux.efi\n/Windows11\nprotocol: efi\npath: boot():/unused-windows.efi\n").unwrap();
        let output = temporary_root.join("official-menu.png");
        run_case(
            root,
            &esp,
            "official",
            "starting installed Limine",
            1,
            Some(&output),
        );
        println!("inspect {}", output.display());
    }
}
