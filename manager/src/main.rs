#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const STATE: &str = "/var/lib/liminescreen";
const INSTALLED: &str = "/usr/local/bin/liminescreenctl";
const HOOK: &str = "/etc/pacman.d/hooks/99-liminescreen.hook";
const LOADER: &str = r"\EFI\liminescreen\liminescreen.efi";
const LABEL: &str = "liminescreen";

#[derive(Serialize, Deserialize)]
struct State {
    esp: PathBuf,
    enabled: bool,
    original_order: Vec<String>,
    entry: String,
    image_sha256: String,
}

fn run(args: &[&str]) -> Result<String> {
    let output = Command::new(args[0]).args(&args[1..]).output()?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn text(path: &Path) -> Result<&str> {
    Ok(path.to_str().ok_or("path must be valid utf8")?)
}

fn timestamp() -> Result<u128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
}

fn atomic_write(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().ok_or("destination has no parent")?;
    fs::create_dir_all(parent)?;
    if path.is_symlink() {
        return Err("refusing to replace a symlink".into());
    }
    let temporary = parent.join(format!(
        ".liminescreen-{}-{}",
        std::process::id(),
        timestamp()?
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if temporary.exists() {
        fs::remove_file(temporary)?;
    }
    result
}

fn validate_image(data: &[u8]) -> Result<()> {
    if data.len() < 128 || &data[..2] != b"MZ" {
        return Err("image is not a PE executable".into());
    }
    let offset = u32::from_le_bytes(data[0x3c..0x40].try_into()?) as usize;
    if offset > data.len().saturating_sub(94) || &data[offset..offset + 4] != b"PE\0\0" {
        return Err("invalid PE header".into());
    }
    let field = |start| u16::from_le_bytes([data[offset + start], data[offset + start + 1]]);
    if (field(4), field(24), field(92)) != (0x8664, 0x20b, 10) {
        return Err("expected an x86_64 UEFI application".into());
    }
    Ok(())
}

fn boot_order(output: &str) -> Result<Vec<String>> {
    let value = output
        .lines()
        .find_map(|line| line.strip_prefix("BootOrder: "))
        .ok_or("firmware has no readable BootOrder")?;
    let order: Vec<String> = value.split(',').map(str::to_ascii_uppercase).collect();
    if order.is_empty()
        || order
            .iter()
            .any(|s| s.len() != 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid BootOrder".into());
    }
    Ok(order)
}

fn find_entry(output: &str) -> Result<Option<String>> {
    let mut found = None;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("Boot") else {
            continue;
        };
        let Some(number) = rest.get(..4) else {
            continue;
        };
        if !number.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let description = rest[4..].trim_start_matches('*').trim_start();
        let lower = description.to_ascii_lowercase();
        if description.split_whitespace().next() == Some(LABEL)
            && (lower.contains(&format!("file({LOADER})").to_ascii_lowercase())
                || lower.contains(&format!("/{LOADER}").to_ascii_lowercase()))
        {
            if found.is_some() {
                return Err("multiple addon entries; refusing ambiguous edits".into());
            }
            found = Some(number.to_ascii_uppercase());
        }
    }
    Ok(found)
}

fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn target_hash(esp: &Path) -> Result<String> {
    Ok(hash(&fs::read(esp.join("EFI/limine/limine_x64.efi"))?))
}

fn save(state: &State) -> Result<()> {
    atomic_write(
        &Path::new(STATE).join("state.json"),
        &serde_json::to_vec_pretty(state)?,
        0o600,
    )
}

fn read_state() -> Result<State> {
    Ok(serde_json::from_slice(&fs::read(
        Path::new(STATE).join("state.json"),
    )?)?)
}

fn install(esp: &Path, image: &Path) -> Result<()> {
    let esp = esp.canonicalize()?;
    if run(&["findmnt", "-n", "-o", "FSTYPE", "--mountpoint", text(&esp)?])? != "vfat" {
        return Err("ESP must be a mounted FAT filesystem".into());
    }
    let official_before = target_hash(&esp)?;
    let data = fs::read(image)?;
    validate_image(&data)?;
    let source = run(&["findmnt", "-n", "-o", "SOURCE", "--mountpoint", text(&esp)?])?;
    let disk = format!("/dev/{}", run(&["lsblk", "-n", "-o", "PKNAME", &source])?);
    let partition = run(&["lsblk", "-n", "-o", "PARTN", &source])?;
    if partition.is_empty() || !partition.bytes().all(|b| b.is_ascii_digit()) {
        return Err("cannot identify the ESP partition".into());
    }
    let current = run(&["efibootmgr", "-v"])?;
    let original_order = boot_order(&current)?;
    let state_path = Path::new(STATE).join("state.json");
    let previous = if state_path.exists() {
        Some(read_state()?)
    } else {
        None
    };
    if previous.as_ref().is_some_and(|state| state.esp != esp) {
        return Err("an installation exists on a different ESP".into());
    }
    let destination = esp.join("EFI/liminescreen/liminescreen.efi");
    if previous.is_none()
        && (destination.exists() || Path::new(INSTALLED).exists() || Path::new(HOOK).exists())
    {
        return Err("existing unowned addon files; refusing to overwrite".into());
    }
    let backup = Path::new(STATE).join(format!("backup-{}", timestamp()?));
    fs::create_dir_all(&backup)?;
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o700))?;
    atomic_write(&backup.join("firmware.txt"), current.as_bytes(), 0o600)?;
    if destination.exists() {
        atomic_write(
            &backup.join("liminescreen.efi"),
            &fs::read(&destination)?,
            0o600,
        )?;
        atomic_write(&backup.join("state.json"), &fs::read(&state_path)?, 0o600)?;
    }
    let mut entry = find_entry(&current)?;
    if entry.is_none() {
        run(&[
            "efibootmgr",
            "--create-only",
            "--disk",
            &disk,
            "--part",
            &partition,
            "--label",
            LABEL,
            "--loader",
            LOADER,
        ])?;
        entry = find_entry(&run(&["efibootmgr", "-v"])?)?;
    }
    let entry = entry.ok_or("firmware did not create the addon entry")?;
    if boot_order(&run(&["efibootmgr"])?)? != original_order {
        return Err("firmware changed BootOrder unexpectedly; see the recovery record".into());
    }
    let mut state = previous.unwrap_or(State {
        esp: esp.clone(),
        enabled: false,
        original_order,
        entry: String::new(),
        image_sha256: String::new(),
    });
    state.entry = entry;
    state.image_sha256 = hash(&data);
    save(&state)?;
    atomic_write(&destination, &data, 0o644)?;
    let executable = std::env::current_exe()?;
    if executable != Path::new(INSTALLED) {
        atomic_write(Path::new(INSTALLED), &fs::read(executable)?, 0o755)?;
    }
    atomic_write(Path::new(HOOK), b"[Trigger]\nOperation = Install\nOperation = Upgrade\nType = Package\nTarget = limine\nTarget = limine-mkinitcpio-hook\nTarget = omarchy\n\n[Action]\nDescription = Verify liminescreen without modifying Limine\nWhen = PostTransaction\nExec = /usr/local/bin/liminescreenctl repair\n", 0o644)?;
    if target_hash(&esp)? != official_before {
        return Err("official Limine changed during installation".into());
    }
    println!(
        "installed entry {}; official limine checksum unchanged: {}",
        state.entry, official_before
    );
    println!("recovery record: {}", backup.display());
    Ok(())
}

fn desired_order(order: &[String], entry: &str, enabled: bool) -> Vec<String> {
    let mut desired = Vec::new();
    if enabled {
        desired.push(entry.to_owned());
    }
    desired.extend(order.iter().filter(|item| item.as_str() != entry).cloned());
    desired
}

fn manage(action: &str) -> Result<()> {
    let mut state = read_state()?;
    let current = run(&["efibootmgr", "-v"])?;
    if find_entry(&current)?.as_deref() != Some(&state.entry) {
        return Err("installed firmware entry is missing or changed".into());
    }
    if action != "disable" {
        target_hash(&state.esp)?;
        let data = fs::read(state.esp.join("EFI/liminescreen/liminescreen.efi"))?;
        if hash(&data) != state.image_sha256 {
            return Err("addon image changed; reinstall before selecting it".into());
        }
    }
    let next = current
        .lines()
        .find_map(|line| line.strip_prefix("BootNext: "));
    if action == "test" {
        if next.is_some_and(|entry| !entry.eq_ignore_ascii_case(&state.entry)) {
            return Err("another BootNext is scheduled; leaving it intact".into());
        }
        run(&["efibootmgr", "--bootnext", &state.entry])?;
        println!(
            "next boot uses {} once; normal boot order is unchanged",
            state.entry
        );
        return Ok(());
    }
    if action != "repair" {
        state.enabled = action == "enable";
        save(&state)?;
    }
    let order = boot_order(&current)?;
    if state.enabled || action == "disable" {
        let desired = desired_order(&order, &state.entry, state.enabled);
        if desired.is_empty() {
            return Err("refusing an empty BootOrder".into());
        }
        if desired != order {
            run(&["efibootmgr", "--bootorder", &desired.join(",")])?;
        }
    }
    if action == "disable" && next.is_some_and(|entry| entry.eq_ignore_ascii_case(&state.entry)) {
        run(&["efibootmgr", "--delete-bootnext"])?;
    }
    println!(
        "persistent addon selection: {}; official target exists",
        state.enabled
    );
    Ok(())
}

fn execute() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| "help".to_owned());
    if action == "--version" || action == "version" {
        println!("liminescreen {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if action == "help" || action == "--help" {
        println!(
            "liminescreenctl setup|install --image FILE [--esp /boot]\nliminescreenctl test|enable|disable|repair"
        );
        return Ok(());
    }
    if !["setup", "install", "test", "enable", "disable", "repair"].contains(&action.as_str()) {
        return Err("unknown action; run liminescreenctl help".into());
    }
    let mut esp = PathBuf::from("/boot");
    let mut image = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--esp" => esp = PathBuf::from(args.next().ok_or("--esp needs a path")?),
            "--image" => image = Some(PathBuf::from(args.next().ok_or("--image needs a path")?)),
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if run(&["id", "-u"])? != "0" {
        return Err("run through sudo or pkexec".into());
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open("/run/lock/boot-partition.lock")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(format!("boot partition is busy: {error}").into()),
        }
    }
    if action == "setup" || action == "install" {
        install(&esp, &image.ok_or("installation requires --image")?)?;
        if action == "setup" {
            manage("test")?;
        }
    } else {
        manage(&action)?;
    }
    Ok(())
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("liminescreen: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_entries_and_order_preserve_other_choices() {
        let original = boot_order("BootCurrent: 0004\nBootOrder: 0004,0000,2001").unwrap();
        let enabled = desired_order(&original, "0010", true);
        assert_eq!(enabled, ["0010", "0004", "0000", "2001"]);
        assert_eq!(desired_order(&enabled, "0010", true), enabled);
        assert_eq!(desired_order(&enabled, "0010", false), original);
        for path in [
            format!("File({LOADER})"),
            format!("HD(1,GPT,example)/{LOADER}"),
        ] {
            let text = format!("Boot0010* liminescreen\t{path}\n");
            assert_eq!(find_entry(&text).unwrap().as_deref(), Some("0010"));
            assert!(find_entry(&(text.clone() + &text)).is_err());
        }
        assert!(boot_order("BootOrder: invalid").is_err());
    }

    #[test]
    fn image_validation_rejects_wrong_architecture_and_truncation() {
        let mut data = vec![0; 256];
        data[..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        data[64..68].copy_from_slice(b"PE\0\0");
        data[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        data[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        data[156..158].copy_from_slice(&10u16.to_le_bytes());
        assert!(validate_image(&data).is_ok());
        assert!(validate_image(&data[..100]).is_err());
        data[68] = 0;
        assert!(validate_image(&data).is_err());
        data[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(validate_image(&data).is_err());
    }
}
