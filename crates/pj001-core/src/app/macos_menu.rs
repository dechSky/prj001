//! macOS NSMenu 상단 menu bar attach — Phase Menu step 1.
//!
//! 설계 출처: `docs/menu-bar-design.md`. 옵션 C (하이브리드):
//! - Apple 표준 selector(About/Hide/Quit/Minimize/...)는 typed action 직접 wire.
//! - pj001 custom 명령(New Tab/Split/Find/...)은 keyEquivalent만 set, 실 동작은
//!   기존 winit keyboard chain. menu item 클릭은 1차 cut에서 nil action (시각만).
//!
//! 6 menu: App / Shell / Edit / View / Window / Help.
//! winit `App::resumed` 이후 한 번 호출. NSApp.mainMenu 교체.

use std::sync::OnceLock;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};
use winit::event_loop::EventLoopProxy;

use crate::app::event::{AppMenuCommand, UserEvent};

/// Localization helper — 시스템 언어가 한국어면 ko, 아니면 en 반환.
/// 우선순위: NSLocale.preferredLanguages 첫 항목 > LANG env > "en" fallback.
/// Codex 8차 권: OnceLock<bool> 캐시 — menu 구성 시 30+번 ObjC 호출 회피.
/// 시스템 locale 변경 후 NSMenu 재구성 안 함 (재시작 시 반영).
static IS_KOREAN: OnceLock<bool> = OnceLock::new();

fn is_korean_locale() -> bool {
    *IS_KOREAN.get_or_init(detect_korean_locale)
}

fn detect_korean_locale() -> bool {
    // 1) NSLocale 우선 — macOS 표준 user preference.
    unsafe {
        let cls = objc2::class!(NSLocale);
        // NSArray<NSString> autoreleased — borrowed, not retained.
        let langs: *mut NSObject = msg_send![cls, preferredLanguages];
        if !langs.is_null() {
            let count: usize = msg_send![langs, count];
            if count > 0 {
                let first: *mut NSObject = msg_send![langs, objectAtIndex: 0usize];
                if !first.is_null() {
                    let cstr: *const std::ffi::c_char = msg_send![first, UTF8String];
                    if !cstr.is_null()
                        && let Ok(s) = std::ffi::CStr::from_ptr(cstr).to_str()
                    {
                        return s.starts_with("ko");
                    }
                }
            }
        }
    }
    // 2) LANG env fallback.
    std::env::var("LANG")
        .ok()
        .map(|v| v.starts_with("ko"))
        .unwrap_or(false)
}

/// menu/UI 문자열 i18n helper. (en, ko) 튜플로 입력 받아 시스템 locale에 맞춰 선택.
fn tr(en: &'static str, ko: &'static str) -> &'static str {
    if is_korean_locale() { ko } else { en }
}

/// menu click → UserEvent::MenuCommand dispatch path. WindowState init 시 set.
/// 외부 module은 `init_menu_proxy`를 통해서만 접근. menu click 발생 시점에 main thread.
/// EventLoopProxy는 Send + Sync.
static MENU_PROXY: OnceLock<EventLoopProxy<UserEvent>> = OnceLock::new();

/// WindowState::new_with_size에서 한 번 호출. 이후 menu click이 이 proxy로 send_event.
/// Codex 6차 개선: set 실패는 stale proxy를 silent 가리지 않고 warn (재초기화/test 가시성).
pub fn init_menu_proxy(proxy: EventLoopProxy<UserEvent>) {
    if MENU_PROXY.set(proxy).is_err() {
        log::warn!("init_menu_proxy: MENU_PROXY already initialized — ignored (stale proxy 위험)");
    }
}

// Custom action handler — Help/Preferences 등 우리 명령. objc2 define_class!로 NSObject 서브
// 클래스 만들고 NSMenuItem.target에 set. selector는 Pj001MenuTarget.{openHelp,openPreferences}.
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "Pj001MenuTarget"]
    #[ivars = ()]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(openHelp:))]
        fn open_help(&self, _sender: *mut NSObject) {
            // README URL — 다른 OS의 menu에선 Help는 docs 페이지. pj001은 repo README.
            spawn_open("https://github.com/dechSky/prj001");
            log::info!("menu: Help → open repo README");
        }

        #[unsafe(method(openPreferences:))]
        fn open_preferences(&self, _sender: *mut NSObject) {
            let Some(home) = std::env::var_os("HOME") else {
                log::warn!("menu: Preferences — HOME unset");
                return;
            };
            let path = std::path::PathBuf::from(home).join(".config/pj001/config.toml");
            // config.toml이 없으면 default를 생성해서 열기 — 사용자가 빈 파일 보지 않게.
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let default_config = "# pj001 config — schema: https://github.com/dechSky/prj001/blob/main/docs/config-schema.md\n\
                    \n\
                    [general]\n\
                    # theme = \"obsidian\"  # aurora | obsidian | vellum | holo | bento | crystal\n\
                    # shell = \"/bin/zsh\"\n\
                    \n\
                    [backdrop]\n\
                    enabled = true\n\
                    \n\
                    [bell]\n\
                    visible = true\n\
                    audible = false\n\
                    \n\
                    [font]\n\
                    # size = 14.0\n\
                    \n\
                    [block]\n\
                    mode = \"auto\"\n";
                if let Err(e) = std::fs::write(&path, default_config) {
                    log::warn!("menu: Preferences default config write failed at {}: {e}", path.display());
                } else {
                    log::info!("menu: Preferences — config.toml not found, created default at {}", path.display());
                }
            }
            spawn_open(path.to_str().unwrap_or(""));
            log::info!("menu: Preferences → open {}", path.display());
        }

        /// macOS NSMenu item click → UserEvent::MenuCommand로 dispatch.
        /// 단일 selector + NSMenuItem.tag로 명령 식별 (옵션 B, design §2).
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, sender: *mut NSObject) {
            if sender.is_null() {
                return;
            }
            let tag: i64 = unsafe { msg_send![sender, tag] };
            let Some(cmd) = AppMenuCommand::from_tag(tag) else {
                log::warn!("menu_action: unknown tag {tag}");
                return;
            };
            let Some(proxy) = MENU_PROXY.get() else {
                log::warn!("menu_action: MENU_PROXY not yet set (early click?)");
                return;
            };
            if proxy.send_event(UserEvent::MenuCommand(cmd)).is_err() {
                log::warn!("menu_action: send_event failed (event loop closed?)");
            } else {
                log::debug!("menu_action: dispatched {:?}", cmd);
            }
        }
    }
);

/// `open` 명령 + Codex 5차 권: zombie 방지 위해 별도 thread에서 wait. open(1)은 launchd로
/// 즉시 종료하지만 parent가 reap 안 하면 zombie 가능성. thread::spawn은 wait 후 종료.
fn spawn_open(arg: &str) {
    let arg = arg.to_string();
    std::thread::spawn(move || {
        if let Ok(mut child) = std::process::Command::new("open").arg(&arg).spawn() {
            let _ = child.wait();
        }
    });
}

// 단일 NSObject instance를 OnceLock에 보관. main thread 전용이라 raw pointer로.
// NSObject는 NSApplication과 동일 lifetime이라 leak OK.
// SAFETY invariant (Codex 5차 권): 이 static은 module-private이고 오직
// `menu_target(MainThreadMarker)` 경유로만 deref. usize Send/Sync 우회는 ObjC object
// thread-affinity를 컴파일러 보호 해제하므로 외부에서 raw로 꺼내지 말 것.
static MENU_TARGET: OnceLock<usize> = OnceLock::new();

fn menu_target(mtm: MainThreadMarker) -> *mut NSObject {
    let ptr = *MENU_TARGET.get_or_init(|| {
        let alloc = mtm.alloc::<MenuTarget>().set_ivars(());
        let target: Retained<MenuTarget> = unsafe { msg_send![super(alloc), init] };
        // Retained → into_raw로 leak (NSApp과 동일 lifetime, drop 안 함).
        Retained::into_raw(target) as usize
    });
    ptr as *mut NSObject
}

/// macOS 상단 menu bar 부착. 호출 시점: winit Window 생성 후 (NSApp 활성 상태).
/// 중복 호출 안전 — setMainMenu가 idempotent하게 교체.
pub fn attach_menu_bar(mtm: MainThreadMarker) {
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let main = NSMenu::new(mtm);
        // Codex 4차 권: setAutoenablesItems(false)로 menu validation 끔 — action이 nil이어도
        // disabled 시각으로 안 보이게. keyEquivalent는 그대로 표시. 사용자가 menu item을
        // 클릭하면 no-op지만 단축키 hint는 정상 제공.
        main.setAutoenablesItems(false);

        // ── App menu (pj001) ────────────────────────────────────────────────────
        let app_menu = NSMenu::new(mtm);
        app_menu.setAutoenablesItems(false);

        add_action_item(
            mtm,
            &app_menu,
            tr("About pj001", "pj001 정보"),
            sel!(orderFrontStandardAboutPanel:),
            "",
            NSEventModifierFlags::empty(),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Preferences — custom target action. ~/.config/pj001/config.toml을 system editor로 open.
        let prefs = make_item(
            mtm,
            tr("Preferences…", "환경설정…"),
            Some(sel!(openPreferences:)),
            ",",
            NSEventModifierFlags::Command,
        );
        let target_ptr = menu_target(mtm);
        let target_obj: &AnyObject = &*(target_ptr as *mut AnyObject);
        prefs.setTarget(Some(target_obj));
        app_menu.addItem(&prefs);

        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Services submenu — Apple 자동 채움.
        let services_item = make_item(
            mtm,
            tr("Services", "서비스"),
            None,
            "",
            NSEventModifierFlags::empty(),
        );
        let services_menu = NSMenu::new(mtm);
        services_item.setSubmenu(Some(&services_menu));
        app.setServicesMenu(Some(&services_menu));
        app_menu.addItem(&services_item);
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        add_action_item(
            mtm,
            &app_menu,
            tr("Hide pj001", "pj001 가리기"),
            sel!(hide:),
            "h",
            NSEventModifierFlags::Command,
        );
        add_action_item(
            mtm,
            &app_menu,
            tr("Hide Others", "기타 가리기"),
            sel!(hideOtherApplications:),
            "h",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        add_action_item(
            mtm,
            &app_menu,
            tr("Show All", "모두 보기"),
            sel!(unhideAllApplications:),
            "",
            NSEventModifierFlags::empty(),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &app_menu,
            tr("Quit pj001", "pj001 종료"),
            sel!(terminate:),
            "q",
            NSEventModifierFlags::Command,
        );

        attach_submenu(mtm, &main, "pj001", &app_menu);

        // ── Shell menu ──────────────────────────────────────────────────────────
        let shell_menu = NSMenu::new(mtm);
        shell_menu.setAutoenablesItems(false);
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("New Window", "새 윈도우"),
            "n",
            NSEventModifierFlags::Command,
            AppMenuCommand::NewWindow,
        ));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("New Pane", "새 분할"),
            "n",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::NewPane,
        ));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("New Tab", "새 탭"),
            "t",
            NSEventModifierFlags::Command,
            AppMenuCommand::NewTab,
        ));
        shell_menu.addItem(&NSMenuItem::separatorItem(mtm));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("Split Vertically", "세로 분할"),
            "d",
            NSEventModifierFlags::Command,
            AppMenuCommand::SplitVertical,
        ));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("Split Horizontally", "가로 분할"),
            "d",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::SplitHorizontal,
        ));
        shell_menu.addItem(&NSMenuItem::separatorItem(mtm));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("Close", "닫기"),
            "w",
            NSEventModifierFlags::Command,
            AppMenuCommand::CloseActive,
        ));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("Close Pane", "분할 닫기"),
            "w",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
            AppMenuCommand::ClosePane,
        ));
        shell_menu.addItem(&make_command_item(
            mtm,
            tr("Close Window", "윈도우 닫기"),
            "w",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::CloseWindow,
        ));
        attach_submenu(mtm, &main, tr("Shell", "셸"), &shell_menu);

        // ── Edit menu ───────────────────────────────────────────────────────────
        let edit_menu = NSMenu::new(mtm);
        edit_menu.setAutoenablesItems(false);
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Copy", "복사"),
            "c",
            NSEventModifierFlags::Command,
            AppMenuCommand::Copy,
        ));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Paste", "붙여넣기"),
            "v",
            NSEventModifierFlags::Command,
            AppMenuCommand::Paste,
        ));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Select All", "전체 선택"),
            "a",
            NSEventModifierFlags::Command,
            AppMenuCommand::SelectAll,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Find…", "찾기…"),
            "f",
            NSEventModifierFlags::Command,
            AppMenuCommand::Find,
        ));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Find Next", "다음 찾기"),
            "g",
            NSEventModifierFlags::Command,
            AppMenuCommand::FindNext,
        ));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Find Previous", "이전 찾기"),
            "g",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::FindPrev,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Clear Buffer", "버퍼 지우기"),
            "k",
            NSEventModifierFlags::Command,
            AppMenuCommand::ClearBuffer,
        ));
        edit_menu.addItem(&make_command_item(
            mtm,
            tr("Clear Scrollback", "스크롤백 지우기"),
            "k",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::ClearScrollback,
        ));
        attach_submenu(mtm, &main, tr("Edit", "편집"), &edit_menu);

        // ── View menu ───────────────────────────────────────────────────────────
        let view_menu = NSMenu::new(mtm);
        view_menu.setAutoenablesItems(false);
        view_menu.addItem(&make_command_item(
            mtm,
            tr("Bigger", "확대"),
            "=",
            NSEventModifierFlags::Command,
            AppMenuCommand::ZoomIn,
        ));
        view_menu.addItem(&make_command_item(
            mtm,
            tr("Smaller", "축소"),
            "-",
            NSEventModifierFlags::Command,
            AppMenuCommand::ZoomOut,
        ));
        view_menu.addItem(&make_command_item(
            mtm,
            tr("Actual Size", "원래 크기"),
            "0",
            NSEventModifierFlags::Command,
            AppMenuCommand::ZoomReset,
        ));
        view_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &view_menu,
            tr("Toggle Full Screen", "전체 화면 전환"),
            sel!(toggleFullScreen:),
            "f",
            NSEventModifierFlags::Command | NSEventModifierFlags::Control,
        );
        attach_submenu(mtm, &main, tr("View", "보기"), &view_menu);

        // ── Window menu (Apple 표준) ────────────────────────────────────────────
        let window_menu = NSMenu::new(mtm);
        window_menu.setAutoenablesItems(false);
        add_action_item(
            mtm,
            &window_menu,
            tr("Minimize", "최소화"),
            sel!(performMiniaturize:),
            "m",
            NSEventModifierFlags::Command,
        );
        add_action_item(
            mtm,
            &window_menu,
            tr("Zoom", "확대/축소"),
            sel!(performZoom:),
            "",
            NSEventModifierFlags::empty(),
        );
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &window_menu,
            tr("Bring All to Front", "모두 앞으로"),
            sel!(arrangeInFront:),
            "",
            NSEventModifierFlags::empty(),
        );
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        window_menu.addItem(&make_command_item(
            mtm,
            tr("Previous Tab", "이전 탭"),
            "\t",
            NSEventModifierFlags::Control | NSEventModifierFlags::Shift,
            AppMenuCommand::PrevTab,
        ));
        window_menu.addItem(&make_command_item(
            mtm,
            tr("Next Tab", "다음 탭"),
            "\t",
            NSEventModifierFlags::Control,
            AppMenuCommand::NextTab,
        ));
        window_menu.addItem(&make_command_item(
            mtm,
            tr("Show Tab Overview", "탭 개요 보기"),
            "\\",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            AppMenuCommand::TabOverview,
        ));
        attach_submenu(mtm, &main, tr("Window", "윈도우"), &window_menu);
        // Apple이 자동으로 윈도우 목록을 windowsMenu에 채움.
        app.setWindowsMenu(Some(&window_menu));

        // ── Help menu ───────────────────────────────────────────────────────────
        let help_menu = NSMenu::new(mtm);
        help_menu.setAutoenablesItems(false);
        let help_item = make_item(
            mtm,
            tr("pj001 Help", "pj001 도움말"),
            Some(sel!(openHelp:)),
            "?",
            NSEventModifierFlags::Command,
        );
        let target_ptr = menu_target(mtm);
        let target_obj: &AnyObject = &*(target_ptr as *mut AnyObject);
        help_item.setTarget(Some(target_obj));
        help_menu.addItem(&help_item);
        attach_submenu(mtm, &main, tr("Help", "도움말"), &help_menu);
        // Apple Help search 활성화.
        app.setHelpMenu(Some(&help_menu));

        // 부착.
        app.setMainMenu(Some(&main));
        log::info!("macos_menu: NSMenu attached (6 menus, hybrid selector + keyEquivalent)");
    }
}

/// `AppMenuCommand` tag + menuAction: selector wire. click → UserEvent dispatch.
/// keyEquivalent 표시는 그대로 — winit keyboard chain과 menu click 둘 다 동일 명령.
unsafe fn make_command_item(
    mtm: MainThreadMarker,
    title: &str,
    key_equivalent: &str,
    modifiers: NSEventModifierFlags,
    cmd: AppMenuCommand,
) -> Retained<NSMenuItem> {
    unsafe {
        let item = make_item(
            mtm,
            title,
            Some(sel!(menuAction:)),
            key_equivalent,
            modifiers,
        );
        let _: () = msg_send![&*item, setTag: cmd as i64];
        let target_ptr = menu_target(mtm);
        let target_obj: &AnyObject = &*(target_ptr as *mut AnyObject);
        item.setTarget(Some(target_obj));
        item
    }
}

/// keyEquivalent만 있는 menu item — action=nil (1차 cut visual only).
unsafe fn make_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key_equivalent: &str,
    modifiers: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    unsafe {
        let title_ns = NSString::from_str(title);
        let key_ns = NSString::from_str(key_equivalent);
        let item = NSMenuItem::new(mtm);
        item.setTitle(&title_ns);
        item.setKeyEquivalent(&key_ns);
        item.setKeyEquivalentModifierMask(modifiers);
        if let Some(sel) = action {
            // action set만 — target은 first responder chain (Apple selector).
            let _: () = msg_send![&*item, setAction: sel];
        }
        item
    }
}

unsafe fn add_action_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Sel,
    key_equivalent: &str,
    modifiers: NSEventModifierFlags,
) {
    unsafe {
        let item = make_item(mtm, title, Some(action), key_equivalent, modifiers);
        menu.addItem(&item);
    }
}

fn attach_submenu(mtm: MainThreadMarker, main: &NSMenu, title: &str, submenu: &NSMenu) {
    let title_ns = NSString::from_str(title);
    submenu.setTitle(&title_ns);
    let item = NSMenuItem::new(mtm);
    item.setTitle(&title_ns);
    item.setSubmenu(Some(submenu));
    main.addItem(&item);
}
