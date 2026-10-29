//! 시작 메뉴 앱 목록 수집: 셸의 AppsFolder(`shell:AppsFolder`)를 열거한다.
//!
//! 일반 데스크톱 앱과 스토어(UWP) 앱이 한 목록에 나오고, 이름은 현재 로캘 기준이다.
//! 각 항목의 파싱 이름을 `shell:AppsFolder\<파싱 이름>` 으로 ShellExecute 하면 실행된다.

use fdpick_core::apps::{is_launchable, AppEntry};
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2, SHGetKnownFolderItem, BHID_EnumItems, KF_FLAG_DEFAULT,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};

unsafe fn display_name(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
    let p: PWSTR = item.GetDisplayName(kind).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

const PKEY_LINK_TARGET_PARSING_PATH: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25), pid: 2 };
const PKEY_LINK_ARGUMENTS: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x436f2667_14e2_4feb_b30a_146c53b5b674), pid: 100 };

fn is_exe(p: &str) -> bool {
    p.to_ascii_lowercase().ends_with(".exe") && std::path::Path::new(p).is_file()
}

/// 관리자 권한으로 실행할 (실행 파일, 인자). 파싱 이름이 실행 파일 경로이거나 바로가기의 대상이
/// 실행 파일일 때만 있다(스토어 앱은 None).
unsafe fn admin_target(item: &IShellItem, parse: &str) -> Option<(String, String)> {
    if is_exe(parse) {
        return Some((parse.to_string(), String::new()));
    }
    let i2: IShellItem2 = item.cast().ok()?;
    let get = |k: &PROPERTYKEY| -> Option<String> {
        let p = i2.GetString(k).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    };
    let path = get(&PKEY_LINK_TARGET_PARSING_PATH)?;
    is_exe(&path).then(|| (path, get(&PKEY_LINK_ARGUMENTS).unwrap_or_default()))
}

/// AppsFolder 를 열거한다. 실패하면 빈 목록(내장 도구는 엔진이 따로 채운다).
pub fn enumerate() -> Vec<AppEntry> {
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let out = enumerate_inner().unwrap_or_default();
        if com {
            CoUninitialize();
        }
        out
    }
}

unsafe fn enumerate_inner() -> windows::core::Result<Vec<AppEntry>> {
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
    let mut out = Vec::new();
    loop {
        let mut slot: [Option<IShellItem>; 1] = [None];
        let mut got = 0u32;
        let hr = items.Next(&mut slot, Some(&mut got));
        if hr.is_err() || got == 0 {
            break;
        }
        let Some(item) = slot[0].take() else { break };
        let (Some(name), Some(parse)) =
            (display_name(&item, SIGDN_NORMALDISPLAY), display_name(&item, SIGDN_DESKTOPABSOLUTEPARSING))
        else {
            continue;
        };
        if name.trim().is_empty() || parse.is_empty() || !is_launchable(&name, &parse) {
            continue;
        }
        let admin = admin_target(&item, &parse);
        // 파싱 이름이 `shell:` 로 시작하지 않으면 AppsFolder 기준 상대 이름이다
        let target = if parse.starts_with("shell:") { parse } else { format!("shell:AppsFolder\\{parse}") };
        out.push(AppEntry { name, aliases: Vec::new(), target, args: String::new(), admin });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// 실제 셸이 필요하다(`cargo test -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn enumerates_apps_folder() {
        let v = super::enumerate();
        for e in v.iter().take(15) {
            println!("{} => {} | admin {:?}", e.name, e.target, e.admin);
        }
        println!("total {}", v.len());
        assert!(!v.is_empty());
    }
}
