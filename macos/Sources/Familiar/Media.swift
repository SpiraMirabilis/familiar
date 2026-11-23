import AVFoundation
import AppKit
import UserNotifications

/// A session's sounds: trigger sounds and the server's (GMCP Client.Media), by name.
final class SoundPlayer: NSObject, AVAudioPlayerDelegate {
    private var playing: [(name: String, player: AVAudioPlayer)] = []
    /// Bumped by every stop, so a sound still downloading when it arrives doesn't start.
    private var stops = 0

    func play(name: String, file: URL, volume: Double, loops: Int, problem: (String) -> Void) {
        do {
            let player = try AVAudioPlayer(contentsOf: file)
            player.volume = Float(min(max(volume, 0), 1))
            // AVAudioPlayer counts repeats after the first play; -1 is forever for both of us.
            player.numberOfLoops = loops < 0 ? -1 : max(loops, 1) - 1
            player.delegate = self
            player.play()
            playing.append((name, player))
        } catch {
            problem("Couldn’t play \(file.lastPathComponent): \(error.localizedDescription)")
        }
    }

    func stop(_ name: String?) {
        stops += 1
        playing.removeAll { sound in
            let match = name == nil || sound.name == name
            if match { sound.player.stop() }
            return match
        }
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        playing.removeAll { $0.player === player }
    }

    /// Runs a Client.Media command from the engine. Downloads happen off the main thread.
    func run(_ command: JSON, problem: @escaping (String) -> Void) {
        let url = command["url"].string
        switch command["op"].string {
        case "stop": stop(command["name"].optionalString)
        case "preload":
            DispatchQueue.global(qos: .utility).async { _ = try? Core.callAnywhere("media.fetch", ["url": .of(url)]) }
        case "play":
            let name = command["name"].string
            if playing.contains(where: { $0.name == name }) {
                if !command["restart"].bool { return }
                stop(name)
            }
            let stopsBefore = stops
            DispatchQueue.global(qos: .userInitiated).async {
                let result = Result { try Core.callAnywhere("media.fetch", ["url": .of(url)]) }
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.stops == stopsBefore else { return }
                    switch result {
                    case .success(let reply):
                        self.play(name: name, file: URL(fileURLWithPath: reply["path"].string), volume: command["volume"].optionalDouble ?? 1, loops: command["loops"].optionalInt ?? 1, problem: problem)
                    case .failure(let error): problem("Couldn’t get the sound: \(error.localizedDescription)")
                    }
                }
            }
        default: break
        }
    }
}

/// Text to speech with the system voice. One for the whole app, so sessions queue their
/// speech rather than talk over each other.
final class Speech {
    static let shared = Speech()
    private let synthesizer = AVSpeechSynthesizer()

    func say(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        synthesizer.speak(AVSpeechUtterance(string: text))
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
    }
}

/// Notification Center banners, for triggers that ask for one.
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = Notifier()
    private var asked = false
    /// Notifications need an app bundle; run straight from the build folder there is none.
    private let available = Bundle.main.bundleIdentifier != nil

    func show(title: String, body: String) {
        guard available else { return }
        let center = UNUserNotificationCenter.current()
        let send = {
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
        if asked { return send() }
        asked = true
        center.delegate = self
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            if granted { send() }
        }
    }

    // Show them even when Familiar is in front: the line may be in another tab.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification, withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .list])
    }
}
