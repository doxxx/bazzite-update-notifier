I use Bazzite as my main desktop OS. One of the issues I have with it though is that when there is an update available, there is no notification of an update being available, and after the update there is no change notes shown to the user. If I wanted to implement something like this and contribute it back to the Bazzite project, how could it be implemented?

---

Contributing an update notification and changelog system to Bazzite is a great idea. Because Bazzite is an OCI-based (containerized) Fedora Atomic derivative, its update mechanism differs from traditional "package-by-package" distributions.

Here is a conceptual roadmap of how this could be implemented and integrated into the project.

---

## 1. Update Detection Mechanism

Since Bazzite uses `rpm-ostree` under the hood, the system needs a lightweight way to check for new image commits without performing a full pull.

* **The Command:** `rpm-ostree upgrade --check` is the standard way to see if a new deployment is available.
* **The Implementation:** A **systemd timer** and a corresponding **service** could run this check periodically (e.g., every 4 to 8 hours).
* **Alternative:** Since Bazzite images are hosted on a container registry (GitHub Container Registry), a script could query the [GitHub API](https://docs.github.com/en/rest) to check for the latest image digest and compare it against the local `IMAGE_ID`.

## 2. Notification System

Bazzite supports multiple desktop environments (KDE Plasma, GNOME, and the Steam Deck's Gamescope). The notification system should be DE-agnostic.

* **Desktop Notifications:** Use `libnotify` (`notify-send`) for a standard toast notification.
* **The Logic:** If the update check finds a new commit, the service triggers a script that sends a notification: *"A new Bazzite system update is available. [View Changelog] [Update Now]"*.
* **Integration:** For the Steam Deck/Handheld UI, this would ideally need to hook into the Steam "System Update" indicator, though that is more complex as it involves Valve's closed-source UI components.

## 3. Generating Change Notes

This is the most requested feature. Because Bazzite is an image, the "changes" are a combination of Fedora base updates and Bazzite-specific tweaks.

* **Fetching Data:** The script can pull the git commit history from the [Bazzite GitHub repository](https://github.com/ublue-os/bazzite).
* **Filtering:** You would likely want to filter for the `main` branch or specific tags.
* **Local Diff:** You can use `rpm-ostree db diff` to compare the currently booted deployment with the pending deployment. This generates a list of added, removed, and upgraded packages, which is highly technical but very accurate.
* **Presentation:** A simple Python or Bash-based GUI (using `yad` or `zenity`) could fetch the latest GitHub Release notes and display them in a scrollable text window.

## 4. Implementation Strategy

If you want to contribute this back, the best place to start is the **Bazzite-arch** or **Bazzite-config** repositories.

### Recommended Tech Stack:

* **Language:** Bash or Python (keep it light to minimize dependencies).
* **Trigger:** A `systemd` user service.
* **UI:** `yad` (already included in many Bazzite builds) for a simple, functional popup window.

### Proposed Workflow:

1. **Check:** Service runs `rpm-ostree upgrade --check`.
2. **Notify:** If `True`, send a notification with an "Action" button.
3. **Display:** If the user clicks "View Changes," a window opens.
* It fetches the last 10 commit messages from the GitHub API.
* It runs `rpm-ostree db diff` to show package changes.


4. **Execute:** Provide a button that triggers the update (e.g., running the `ujust update` command).

---

### How to Start Contributing

Before writing code, it is highly recommended to open an **Issue** or a **Discussions** thread on the [Bazzite GitHub](https://github.com/ublue-os/bazzite/issues) or join the **Universal Blue Discord**.

The maintainers can provide guidance on where they want the binaries stored and how to ensure the notification doesn't interrupt users in full-screen gaming mode.

---

CachyOS has an update notifier that sites in the system tray. Is it possible to do this in a way that works for both KDE and Gnome?

---

Yes, it is absolutely possible to create a tray-based update notifier that works across both KDE Plasma and GNOME. In fact, many cross-distro tools (like Dropbox, Steam, or Discord) use a shared standard to achieve this.

Given that you are on Bazzite in **2026** (running Fedora 44 and GNOME 50+), you'll be targeting a Wayland-first environment where legacy "System Tray" protocols have been replaced by modern standards.

---

## 1. The Protocol: StatusNotifierItem (SNI)

To work on both KDE and GNOME, your app should implement the **StatusNotifierItem (SNI)** protocol (often referred to as **AppIndicators**).

* **KDE Plasma:** Supports SNI natively. The icon will appear in the system tray automatically.
* **GNOME:** Does **not** support tray icons natively in the shell. However, Bazzite (and most Fedora-based gaming distros) usually pre-installs or recommends the **"AppIndicator and KStatusNotifierItem Support"** extension. This extension acts as a bridge, allowing SNI icons to appear in the GNOME top bar.

## 2. Recommended Implementation: Python + `pystray`

For a Bazzite contribution, Python is an excellent choice because it’s already in the base image, easy to maintain, and has great libraries for this.

### The "Tray" Logic

You can use `pystray` to create the icon and menu. It can use the `libappindicator` or `dbus` backend, which is the most compatible way to talk to both KDE and GNOME.

```python
import pystray
from PIL import Image

def on_clicked(icon, item):
    if str(item) == "Update Now":
        # Trigger your update script
        pass

icon = pystray.Icon("BazziteUpdater")
icon.menu = pystray.Menu(
    pystray.MenuItem("New Update Available!", None, enabled=False),
    pystray.MenuItem("View Changelog", on_clicked),
    pystray.MenuItem("Update Now", on_clicked)
)
icon.icon = Image.open("bazzite_icon.png")
icon.run()

```

## 3. Integrating with Bazzite (OCI/Atomic)

Since Bazzite is an atomic system, your implementation needs to hook into `rpm-ostree`.

### The Update Check

Instead of a heavy background process, use a **systemd user timer**.

* **Timer:** Runs every 4 hours.
* **Service:** Executes `rpm-ostree upgrade --check`.
* **Signal:** If an update is found, the service sends a signal (via D-Bus or a simple flag file) to your tray application to change the icon color or show a notification.

### Generating "Change Notes"

This is where you can really improve the user experience. Since Bazzite is an image, you can't just read a simple text file. You can generate a diff:

```bash
# Compare the booted deployment with the pending one
rpm-ostree db diff

```

Your Python tray app could parse this output to show exactly which packages (like the Kernel, Mesa, or Gamescope) are changing.

## 4. How to Contribute to Bazzite

Bazzite's philosophy is "batteries included but lightweight." To get this accepted:

1. **Keep it DE-Agnostic:** Do not use `KDE` or `GTK` specific libraries if possible. Stick to D-Bus for notifications and SNI for the icon.
2. **Package it as a Systemd User Service:** Bazzite prefers managing background tasks through systemd.
3. **Target `bazzite-config`:** Most of the "glue" code for Bazzite lives in the [ublue-os/bazzite-config](https://github.com/ublue-os/bazzite) repository. You would likely contribute a Python script, a `.desktop` file for autostart, and the systemd units.

---

### Comparison with CachyOS

CachyOS uses a Qt-based tool (`cachy-update`). While Qt is great for KDE, it can pull in many dependencies on a GNOME system. By using a small **Python/SNI** script, you keep the "Atomic" image slim while providing the same visual feedback.

---

Bazzite also posts more human readable update notes to their Discourse site. Is there an API to pull that post for display or perhaps just find the link so that it can be displayed in the user's browser?

---

Yes, Bazzite (and the wider Universal Blue project) uses **Discourse** for their community forum, and Discourse has a remarkably clean and open API.

Every URL on a Discourse site can be turned into a JSON endpoint simply by appending `.json` to it. This makes it very easy to fetch the latest "human-readable" update notes programmatically.

### 1. The Data Source

Bazzite update notes are typically posted to the [Universal Blue Discourse](https://universal-blue.discourse.group/) under the **Bazzite** category, specifically tagged with `announcements` or `bazzite-news`.

* **Human URL:** `[https://universal-blue.discourse.group/tag/bazzite-news](https://universal-blue.discourse.group/tag/bazzite-news)`
* **API URL:** `[https://universal-blue.discourse.group/tag/bazzite-news.json](https://universal-blue.discourse.group/tag/bazzite-news.json)`

### 2. Implementation Logic

If you were to implement this in a Python-based tray notifier, your logic would look like this:

1. **Fetch the News Feed:** Call the `.json` endpoint for the `bazzite-news` tag.
2. **Parse the Latest Topic:** The JSON response contains a list of `topics`. The first one is usually the most recent.
3. **Check for "Unread" Status:** You could store the ID of the last seen announcement locally (e.g., in `~/.config/bazzite-updater/last_id`). If the latest ID on Discourse is newer, trigger a notification.
4. **Extract the Link:** The JSON provides a `slug` and an `id`. You can reconstruct the full URL: `[https://universal-blue.discourse.group/t/](https://universal-blue.discourse.group/t/){slug}/{id}`.

### 3. Displaying Content

You have two options for showing the notes to the user:

* **Option A (The Browser):** This is the easiest. When the user clicks "View Changelog" in your tray menu, just run:
```python
import webbrowser
webbrowser.open(f"https://universal-blue.discourse.group/t/{slug}/{id}")

```


* **Option B (In-App Display):** If you want to show the text inside a window (like CachyOS does), you can fetch the specific topic data:
* **Topic API:** `[https://universal-blue.discourse.group/t/](https://universal-blue.discourse.group/t/){id}.json`
* This returns a `post_stream` object. The first post's `cooked` field contains the HTML of the update notes. You can render this HTML in a simple Gtk or Qt WebKit/WebEngine view.



### 4. A Note on "Bazaar"

In the current **Bazzite 44** (May 2026) release, the project has been moving toward a tool called **Bazaar** (often used for managing game-specific tweaks). There has been internal discussion about integrating news feeds directly into a "Bazzite Portal" or "Bazaar" interface.

If you want your contribution to be officially merged:

* **Look into the `bazzite-portal` repository.** This is the GUI tool Bazzite uses for initial setup. Adding a "What's New" tab there that pulls from the Discourse API would be a very "native" feeling contribution.
* **Use the SBOMs.** As of 2026, Bazzite is also using **Software Bill of Materials (SBOMs)** to power technical changelogs. You could potentially combine the *human* Discourse post with a *technical* SBOM diff to give users the best of both worlds.

---

### Pro-Tip for your tray app:

Since Bazzite updates can be large (it's a whole image download), it's good practice to show the **Discourse Title** in the notification itself.
*Example:*

> **Update Available: Bazzite 44 - The "Spring Cleaning" Update**
> [Read Notes] [Update Now]
