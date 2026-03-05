# Terminal App Architecture Guide

This document provides an overview of the code structure for the Android Terminal application, focusing on the new architecture located in the [`new2`] package and the application entry point.

## Entry Point

### [`LauncherActivity`]
[`java/com/android/virtualization/terminal/LauncherActivity.kt`]

The [`LauncherActivity`] serves as the main entry point for the application. Its primary responsibility is to route the user to the appropriate UI implementation based on the system property `ro.terminal.new_ui.enabled`.
-   **Old UI**: Redirects to [`com.android.virtualization.terminal.MainActivity`].
-   **New UI**: Redirects to [`com.android.virtualization.terminal.new2.ui.MainActivity`].

You need to enable the new feature flag to use the new UI:

```
adb root
adb shell aflags enable com.android.system.virtualmachine.flags.terminal_newui_jetpack
adb reboot
```

## New Architecture ([`new2`] package)

The [`com.android.virtualization.terminal.new2`] package contains the refactored application architecture, which utilizes **Jetpack Compose** for the UI and **Kotlin Coroutines/Flow** for state management.

### Core Components ([`new2/core`])

This package contains the core business logic, state management, and service integration.

*   **[`VmController`]** ([`VmController.kt`])
    *   A singleton object that manages the lifecycle of the Virtual Machine (VM).
    *   Interacts with the `VirtualMachineManager` system service to create, start, and stop the VM.
    *   Uses `NsdManager` to discover the port where the terminal service (`ttyd`) is running inside the VM.
    *   Exposes a `vmState` (`StateFlow<VmState>`) to observe changes in VM status (Ready, Starting, Running, Stopped, Error).

*   **[`Installer`]** ([`Installer.kt`])
    *   A singleton object responsible for downloading and installing the Linux VM image.
    *   Manages the installation process in the background.
    *   Exposes an `installState` (`StateFlow<InstallState>`) to track progress (Checking, NotInstalled, Installing, Installed, Error).

*   **[`VmService`]** ([`VmService.kt`])
    *   A `ForegroundService` that ensures critical operations continue running even when the application is in the background.
    *   Handles actions for installing the image (`ACTION_INSTALL`), starting the VM (`ACTION_START`), and stopping the VM (`ACTION_STOP`).
    *   Delegates actual logic to [`Installer`] and [`VmController`].

*   **[`TtydView`]** ([`TtydView.kt`])
    *   A custom `WebView` implementation (extending `TerminalView`) that renders the terminal interface.
    *   Connects to the `ttyd` service running inside the guest VM.
    *   Handles SSL/TLS connections, client certificate authentication, and JavaScript injection to bridge `xterm.js` events with Android.

*   **[`InstallState`]** & **[`VmState`]**
    *   Sealed interfaces defining the finite states for the installation process and the VM lifecycle, respectively.

### UI Layer ([`new2/ui`])

The UI layer is built using Jetpack Compose and follows the MVVM (Model-View-ViewModel) pattern.

*   **[`MainActivity`]** ([`ui/MainActivity.kt`])
    *   The container Activity for the new Compose-based UI.
    *   Sets up the `MaterialTheme` and hosts the [`MainScreen`].

*   **[`MainScreen`]** ([`ui/MainScreen.kt`])
    *   The top-level Composable function.
    *   Observes the `MainUiState` from [`MainViewModel`] and orchestrates navigation between different screens:
        *   `SplashScreen`: Loading state.
        *   `InstallStartScreen`: Prompts user to install the VM image.
        *   `InstallProgressScreen`: Shows download/installation progress.
        *   `TerminalScreen`: Displays the running terminal.
        *   `BootingScreen`: Shows VM boot progress.

### ViewModels ([`new2/ui/main`])

*   **[`MainViewModel`]** ([`ui/main/MainViewModel.kt`])
    *   The primary ViewModel for the application.
    *   Combines [`Installer`].`installState` and [`VmController`].`vmState` into a single, unified `MainUiState`.
    *   Exposes methods for UI actions like `installVm()`, `startVm()`, and `stopVm()`.

*   **[`TerminalViewModel`]** ([`ui/main/TerminalViewModel.kt`])
    *   Manages the state of the terminal view (`TerminalUiState`).
    *   Responsible for creating and managing the lifecycle of the [`TtydView`] instance, ensuring it is attached to the correct `DisplayContext`.

### Utilities ([`new2/util`])

*   **[`LoggingMutableStateFlow`]** ([`util/LoggingMutableStateFlow.kt`])
    *   A utility wrapper around `MutableStateFlow` that logs all state changes to logcat. This is useful for debugging state transitions in the reactive architecture.

[`InstallState`]: java/com/android/virtualization/terminal/new2/core/InstallState.kt
[`Installer.kt`]: java/com/android/virtualization/terminal/new2/core/Installer.kt
[`Installer`]: java/com/android/virtualization/terminal/new2/core/Installer.kt
[`LauncherActivity`]: java/com/android/virtualization/terminal/LauncherActivity.kt
[`LoggingMutableStateFlow`]: java/com/android/virtualization/terminal/new2/util/LoggingMutableStateFlow.kt
[`MainActivity`]: java/com/android/virtualization/terminal/new2/ui/MainActivity.kt
[`MainScreen`]: java/com/android/virtualization/terminal/new2/ui/MainScreen.kt
[`MainViewModel`]: java/com/android/virtualization/terminal/new2/ui/main/MainViewModel.kt
[`TerminalViewModel`]: java/com/android/virtualization/terminal/new2/ui/main/TerminalViewModel.kt
[`TtydView.kt`]: java/com/android/virtualization/terminal/new2/core/TtydView.kt
[`TtydView`]: java/com/android/virtualization/terminal/new2/core/TtydView.kt
[`VmController.kt`]: java/com/android/virtualization/terminal/new2/core/VmController.kt
[`VmController`]: java/com/android/virtualization/terminal/new2/core/VmController.kt
[`VmService.kt`]: java/com/android/virtualization/terminal/new2/core/VmService.kt
[`VmService`]: java/com/android/virtualization/terminal/new2/core/VmService.kt
[`VmState`]: java/com/android/virtualization/terminal/new2/core/VmState.kt
[`com.android.virtualization.terminal.MainActivity`]: java/com/android/virtualization/terminal/MainActivity.kt
[`com.android.virtualization.terminal.new2.ui.MainActivity`]: java/com/android/virtualization/terminal/new2/ui/MainActivity.kt
[`com.android.virtualization.terminal.new2`]: java/com/android/virtualization/terminal/new2
[`java/com/android/virtualization/terminal/LauncherActivity.kt`]: java/com/android/virtualization/terminal/LauncherActivity.kt
[`new2/core`]: java/com/android/virtualization/terminal/new2/core
[`new2/ui/main`]: java/com/android/virtualization/terminal/new2/ui/main
[`new2/ui`]: java/com/android/virtualization/terminal/new2/ui
[`new2/util`]: java/com/android/virtualization/terminal/new2/util
[`new2`]: java/com/android/virtualization/terminal/new2
[`ui/MainActivity.kt`]: java/com/android/virtualization/terminal/new2/ui/MainActivity.kt
[`ui/MainScreen.kt`]: java/com/android/virtualization/terminal/new2/ui/MainScreen.kt
[`ui/main/MainViewModel.kt`]: java/com/android/virtualization/terminal/new2/ui/main/MainViewModel.kt
[`ui/main/TerminalViewModel.kt`]: java/com/android/virtualization/terminal/new2/ui/main/TerminalViewModel.kt
[`util/LoggingMutableStateFlow.kt`]: java/com/android/virtualization/terminal/new2/util/LoggingMutableStateFlow.kt
