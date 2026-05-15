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
            spawn_open(path.to_str().unwrap_or(""));
            log::info!("menu: Preferences → open {}", path.display());
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
        let app_menu = NSMenu::new(mtm); app_menu.setAutoenablesItems(false);

        add_action_item(
            mtm,
            &app_menu,
            "About pj001",
            sel!(orderFrontStandardAboutPanel:),
            "",
            NSEventModifierFlags::empty(),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Preferences — custom target action. ~/.config/pj001/config.toml을 system editor로 open.
        let prefs = make_item(
            mtm,
            "Preferences…",
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
            "Services",
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
            "Hide pj001",
            sel!(hide:),
            "h",
            NSEventModifierFlags::Command,
        );
        add_action_item(
            mtm,
            &app_menu,
            "Hide Others",
            sel!(hideOtherApplications:),
            "h",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        add_action_item(
            mtm,
            &app_menu,
            "Show All",
            sel!(unhideAllApplications:),
            "",
            NSEventModifierFlags::empty(),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &app_menu,
            "Quit pj001",
            sel!(terminate:),
            "q",
            NSEventModifierFlags::Command,
        );

        attach_submenu(mtm, &main, "pj001", &app_menu);

        // ── Shell menu ──────────────────────────────────────────────────────────
        let shell_menu = NSMenu::new(mtm); shell_menu.setAutoenablesItems(false);
        // New Window — placeholder (multi-window milestone 전).
        shell_menu.addItem(&make_item(
            mtm,
            "New Window",
            None,
            "n",
            NSEventModifierFlags::Command,
        ));
        shell_menu.addItem(&make_item(
            mtm,
            "New Tab",
            None,
            "t",
            NSEventModifierFlags::Command,
        ));
        shell_menu.addItem(&NSMenuItem::separatorItem(mtm));
        shell_menu.addItem(&make_item(
            mtm,
            "Split Vertically",
            None,
            "d",
            NSEventModifierFlags::Command,
        ));
        shell_menu.addItem(&make_item(
            mtm,
            "Split Horizontally",
            None,
            "d",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        shell_menu.addItem(&NSMenuItem::separatorItem(mtm));
        shell_menu.addItem(&make_item(
            mtm,
            "Close",
            None,
            "w",
            NSEventModifierFlags::Command,
        ));
        shell_menu.addItem(&make_item(
            mtm,
            "Close Tab",
            None,
            "w",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        attach_submenu(mtm, &main, "Shell", &shell_menu);

        // ── Edit menu ───────────────────────────────────────────────────────────
        let edit_menu = NSMenu::new(mtm); edit_menu.setAutoenablesItems(false);
        edit_menu.addItem(&make_item(
            mtm,
            "Copy",
            None,
            "c",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&make_item(
            mtm,
            "Paste",
            None,
            "v",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&make_item(
            mtm,
            "Select All",
            None,
            "a",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&make_item(
            mtm,
            "Find…",
            None,
            "f",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&make_item(
            mtm,
            "Clear Buffer",
            None,
            "k",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&make_item(
            mtm,
            "Clear Scrollback",
            None,
            "k",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        attach_submenu(mtm, &main, "Edit", &edit_menu);

        // ── View menu ───────────────────────────────────────────────────────────
        let view_menu = NSMenu::new(mtm); view_menu.setAutoenablesItems(false);
        view_menu.addItem(&make_item(
            mtm,
            "Bigger",
            None,
            "=",
            NSEventModifierFlags::Command,
        ));
        view_menu.addItem(&make_item(
            mtm,
            "Smaller",
            None,
            "-",
            NSEventModifierFlags::Command,
        ));
        view_menu.addItem(&make_item(
            mtm,
            "Actual Size",
            None,
            "0",
            NSEventModifierFlags::Command,
        ));
        view_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &view_menu,
            "Toggle Full Screen",
            sel!(toggleFullScreen:),
            "f",
            NSEventModifierFlags::Command | NSEventModifierFlags::Control,
        );
        attach_submenu(mtm, &main, "View", &view_menu);

        // ── Window menu (Apple 표준) ────────────────────────────────────────────
        let window_menu = NSMenu::new(mtm); window_menu.setAutoenablesItems(false);
        add_action_item(
            mtm,
            &window_menu,
            "Minimize",
            sel!(performMiniaturize:),
            "m",
            NSEventModifierFlags::Command,
        );
        add_action_item(
            mtm,
            &window_menu,
            "Zoom",
            sel!(performZoom:),
            "",
            NSEventModifierFlags::empty(),
        );
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_action_item(
            mtm,
            &window_menu,
            "Bring All to Front",
            sel!(arrangeInFront:),
            "",
            NSEventModifierFlags::empty(),
        );
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        // Tab navigation — winit chain에 이미 매핑됨 (PrevTab/NextTab). keyEquivalent만 표시.
        window_menu.addItem(&make_item(
            mtm,
            "Previous Tab",
            None,
            "[",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        window_menu.addItem(&make_item(
            mtm,
            "Next Tab",
            None,
            "]",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        attach_submenu(mtm, &main, "Window", &window_menu);
        // Apple이 자동으로 윈도우 목록을 windowsMenu에 채움.
        app.setWindowsMenu(Some(&window_menu));

        // ── Help menu ───────────────────────────────────────────────────────────
        let help_menu = NSMenu::new(mtm); help_menu.setAutoenablesItems(false);
        let help_item = make_item(
            mtm,
            "pj001 Help",
            Some(sel!(openHelp:)),
            "?",
            NSEventModifierFlags::Command,
        );
        let target_ptr = menu_target(mtm);
        let target_obj: &AnyObject = &*(target_ptr as *mut AnyObject);
        help_item.setTarget(Some(target_obj));
        help_menu.addItem(&help_item);
        attach_submenu(mtm, &main, "Help", &help_menu);
        // Apple Help search 활성화.
        app.setHelpMenu(Some(&help_menu));

        // 부착.
        app.setMainMenu(Some(&main));
        log::info!("macos_menu: NSMenu attached (6 menus, hybrid selector + keyEquivalent)");
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
