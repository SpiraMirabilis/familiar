import AppKit
import SwiftUI
import UniformTypeIdentifiers

// MARK: The world map

private struct MapExit {
    let command: String
    let to: Int?
}

private struct MapRoom {
    let id: Int
    let name: String
    let area: String
    var x: Double
    var y: Double
    let w: Double
    let h: Double
    let color: NSColor?
    /// Whether the room's own colour is dark enough to want light text.
    let darkFill: Bool
    let exits: [MapExit]

    init(_ json: JSON, defaultW: Double, defaultH: Double) {
        id = json["id"].int
        name = json["name"].string
        area = json["area"].string
        x = json["x"].double
        y = json["y"].double
        w = json["w"].optionalDouble ?? defaultW
        h = json["h"].optionalDouble ?? defaultH
        color = json["color"].optionalString.flatMap { NSColor(hex: $0) }
        if let color {
            darkFill = 0.2126 * color.redComponent + 0.7152 * color.greenComponent + 0.0722 * color.blueComponent < 0.5
        } else {
            darkFill = false
        }
        exits = json["exits"].array.map { MapExit(command: $0["command"].string, to: $0["to"].optionalInt) }
    }

    var rect: CGRect { CGRect(x: x, y: y, width: w, height: h) }
    var center: CGPoint { CGPoint(x: x + w / 2, y: y + h / 2) }
}

/// Grid offsets for compass directions, as in crates/map/src/directions.rs. The bridge doesn't
/// expose them, and they're needed to draw stubs for exits that lead off the shown area.
private let directionOffsets: [String: (Double, Double)] = {
    let table: [(String, String, (Double, Double))] = [
        ("n", "north", (0, -1)), ("s", "south", (0, 1)), ("e", "east", (1, 0)), ("w", "west", (-1, 0)),
        ("ne", "northeast", (1, -1)), ("nw", "northwest", (-1, -1)), ("se", "southeast", (1, 1)), ("sw", "southwest", (-1, 1)),
        ("u", "up", (0.5, -1)), ("d", "down", (-0.5, 1)), ("in", "in", (1, 0.5)), ("out", "out", (-1, -0.5)),
    ]
    var offsets: [String: (Double, Double)] = [:]
    for (short, long, offset) in table {
        offsets[short] = offset
        offsets[long] = offset
    }
    return offsets
}()

/// A session's map as the panel shows it, and where the view is looking. It outlives the panel
/// (the side dock rebuilds its content when tabs change), so the pan, zoom and area survive.
private final class MapModel: ObservableObject {
    private static let models = NSMapTable<Session, MapModel>.weakToStrongObjects()

    static func model(for session: Session) -> MapModel {
        if let model = models.object(forKey: session) { return model }
        let model = MapModel(session)
        models.setObject(model, forKey: session)
        return model
    }

    private weak var session: Session?
    weak var canvas: MapCanvas? {
        didSet { canvas?.needsDisplay = true }
    }

    @Published private(set) var areas: [String] = []
    @Published var area = "" {
        didSet {
            if area != oldValue { reshow() }
        }
    }
    @Published var follow = true {
        didSet {
            if follow, !oldValue, let current, index[current] != nil {
                shownCurrent = current
                center(on: current)
            }
        }
    }

    private(set) var rooms: [MapRoom] = []
    private(set) var index: [Int: Int] = [:]
    /// Indices into `rooms` of the ones in the shown area, in drawing order.
    private(set) var shown: [Int] = []
    private(set) var current: Int?
    private(set) var selected: Int?
    private(set) var gridX = 144.0
    private(set) var gridY = 72.0
    private var roomW = 96.0
    private var roomH = 32.0

    /// Map units at the view's top-left corner.
    private(set) var origin = CGPoint(x: -40, y: -40)
    private(set) var zoom = 1.0
    private var shownCurrent: Int?
    private var loadedRevision = -1
    /// A room being dragged; the engine hears about it when the drag ends.
    private var dragged: (id: Int, x: Double, y: Double)?

    private init(_ session: Session) {
        self.session = session
    }

    // MARK: Loading

    func reloadIfNeeded() {
        if session?.mapRevision != loadedRevision { reload() }
    }

    /// Call after the map changes: refetches it, updates the area list and follows the current room.
    func reload() {
        guard let session else { return }
        loadedRevision = session.mapRevision
        let map = session.map()
        gridX = map["grid_x"].optionalDouble ?? gridX
        gridY = map["grid_y"].optionalDouble ?? gridY
        roomW = map["room_w"].optionalDouble ?? roomW
        roomH = map["room_h"].optionalDouble ?? roomH
        rooms = map["rooms"].array.map { MapRoom($0, defaultW: roomW, defaultH: roomH) }
        index = Dictionary(rooms.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { _, last in last })
        if let dragged, let i = index[dragged.id] {
            rooms[i].x = dragged.x
            rooms[i].y = dragged.y
        }
        current = map["current"].optionalInt
        if let id = selected, index[id] == nil { selected = nil }

        let names = map["areas"].array.map(\.string)
        if names != areas {
            areas = names
            if !names.contains(area) { area = names.first ?? "" }
        }
        reshow()
        if follow, let current, index[current] != nil, shownCurrent != current {
            shownCurrent = current
            center(on: current)
        }
    }

    private func reshow() {
        shown = rooms.indices.filter { rooms[$0].area == area }
        canvas?.needsDisplay = true
        canvas?.describe()
    }

    func room(_ id: Int) -> MapRoom? {
        index[id].map { rooms[$0] }
    }

    /// The topmost room of the shown area at a point in map units.
    func room(at point: CGPoint) -> MapRoom? {
        // Later rooms are drawn on top, so search from the end.
        for i in shown.reversed() where rooms[i].rect.contains(point) {
            return rooms[i]
        }
        return nil
    }

    // MARK: Looking around

    func toMap(_ point: CGPoint) -> CGPoint {
        CGPoint(x: origin.x + point.x / zoom, y: origin.y + point.y / zoom)
    }

    func pan(to origin: CGPoint) {
        self.origin = origin
        canvas?.needsDisplay = true
    }

    /// Zooms keeping the map under `point` (in view coordinates; the middle of the view if nil) where it is.
    func zoom(by factor: Double, about point: CGPoint? = nil) {
        let size = canvas?.bounds.size ?? .zero
        let point = point ?? CGPoint(x: size.width / 2, y: size.height / 2)
        let before = toMap(point)
        zoom = min(max(zoom * factor, 0.2), 4)
        origin = CGPoint(x: before.x - point.x / zoom, y: before.y - point.y / zoom)
        canvas?.needsDisplay = true
    }

    func select(_ id: Int?) {
        selected = id
        canvas?.needsDisplay = true
    }

    /// Shows the room's area and centres the view on it.
    func center(on id: Int) {
        guard let room = room(id) else { return }
        if areas.contains(room.area) { area = room.area }
        let size = canvas?.bounds.size ?? .zero
        // Before the view is laid out there's nothing to centre in; canvasResized puts it right.
        sizeForOrigin = size
        origin = CGPoint(x: room.center.x - size.width / 2 / zoom, y: room.center.y - size.height / 2 / zoom)
        canvas?.needsDisplay = true
    }

    /// The size the origin was last worked out for.
    private var sizeForOrigin = CGSize.zero

    /// Keeps whatever is in the middle of the view in the middle as it changes size.
    func canvasResized() {
        guard let new = canvas?.bounds.size, new != sizeForOrigin else { return }
        origin.x -= (new.width - sizeForOrigin.width) / 2 / zoom
        origin.y -= (new.height - sizeForOrigin.height) / 2 / zoom
        sizeForOrigin = new
        canvas?.needsDisplay = true
    }

    func centerOnCurrent() {
        if let current { center(on: current) }
    }

    func search(_ text: String) {
        guard let session, !text.isEmpty else { return }
        let found = Core.send("map.search", ["id": .int(session.id), "text": .of(text)]).array
        guard let id = found.first?.optionalInt else { return NSSound.beep() }
        selected = id
        center(on: id)
    }

    // MARK: Changing the map

    func walk(to id: Int) {
        session?.mapCall("map.walk", ["room": .int(id)])
    }

    func setCurrent(_ id: Int) {
        session?.mapCall("map.set_current", ["room": .int(id)])
    }

    func delete(_ id: Int) {
        session?.mapCall("map.delete", ["room": .int(id)])
    }

    /// Moves a room on screen only, while it's being dragged.
    func drag(_ id: Int, to x: Double, _ y: Double) {
        guard let i = index[id] else { return }
        rooms[i].x = x
        rooms[i].y = y
        dragged = (id, x, y)
        canvas?.needsDisplay = true
    }

    func endDrag() {
        guard let moved = dragged else { return }
        dragged = nil
        session?.mapCall("map.move", ["room": .int(moved.id), "x": .double(moved.x), "y": .double(moved.y)])
    }

    /// Adds a room centred on a point in map units, then asks what to call it.
    func addRoom(at point: CGPoint) {
        guard let session else { return }
        let reply = session.mapCall("map.add", [
            "name": .of("Room \(rooms.count + 1)"),
            "area": .of(area),
            "x": .double(point.x - roomW / 2),
            "y": .double(point.y - roomH / 2),
        ])
        guard let id = reply["room"].optionalInt else { return }
        // The panel reloads on its next update; the new room is needed now, to select and rename.
        reload()
        select(id)
        rename(id)
    }

    func rename(_ id: Int) {
        guard let room = room(id) else { return }
        let alert = NSAlert()
        alert.messageText = "Rename Room"
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(string: room.name)
        field.frame = NSRect(x: 0, y: 0, width: 260, height: 24)
        field.setAccessibilityLabel("Room name")
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        let answer: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard response == .alertFirstButtonReturn, !name.isEmpty else { return }
            self?.session?.mapCall("map.rename", ["room": .int(id), "name": .of(name)])
        }
        if let window = session?.window {
            alert.beginSheetModal(for: window, completionHandler: answer)
        } else {
            answer(alert.runModal())
        }
    }

    func importMap() {
        let panel = NSOpenPanel()
        panel.title = "Import BeipMU Map"
        panel.message = "Choose a BeipMU map (.atlas) to import."
        panel.allowedContentTypes = [UTType(filenameExtension: "atlas")].compactMap { $0 }
        let answer: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard response == .OK, let path = panel.url?.path else { return }
            self?.session?.mapCall("map.import", ["path": .of(path)])
        }
        if let window = session?.window {
            panel.beginSheetModal(for: window, completionHandler: answer)
        } else {
            answer(panel.runModal())
        }
    }
}

private final class ActionItem: NSMenuItem {
    private let run: () -> Void

    init(_ title: String, _ run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }

    required init(coder: NSCoder) { fatalError("not in a nib") }

    @objc private func fire() { run() }
}

/// Draws the shown area and handles the mouse: drag a room to move it, drag elsewhere (or
/// scroll) to pan, pinch or turn a wheel to zoom, double-click to walk, right-click for a menu.
private final class MapCanvas: NSView {
    var model: MapModel?

    private enum Drag {
        case none
        case pan(origin: CGPoint, from: CGPoint)
        /// `grab` is where in the room the pointer took hold, in map units.
        case room(id: Int, grab: CGPoint, from: CGPoint, moved: Bool)
    }
    private var drag = Drag.none

    private static let fill = NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.22, green: 0.24, blue: 0.30, alpha: 1)
            : NSColor(srgbRed: 0.86, green: 0.89, blue: 0.95, alpha: 1)
    }
    private static let hint = "No rooms yet.\nThe map fills in as you move if the game sends GMCP Room.Info;\notherwise use /map addroom \"Name\" or right-click to add rooms."

    override init(frame: NSRect) {
        super.init(frame: frame)
        // Layer-backed views don't clip by default, and the map is usually bigger than the view.
        clipsToBounds = true
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel("Map")
    }

    required init?(coder: NSCoder) { fatalError("not in a nib") }

    // Top-left origin with y down, like the map's own coordinates.
    override var isFlipped: Bool { true }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        model?.canvasResized()
    }

    /// Says what's shown, for VoiceOver.
    func describe() {
        guard let model else { return }
        if model.rooms.isEmpty { return setAccessibilityValue("No rooms yet") }
        var text = "\(model.shown.count) rooms in \(model.area.isEmpty ? "no area" : model.area)"
        if let room = model.current.flatMap(model.room) { text += "; you are in \(room.name)" }
        setAccessibilityValue(text)
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        guard let model, let context = NSGraphicsContext.current?.cgContext else { return }
        if model.rooms.isEmpty { return drawHint() }

        let z = model.zoom
        let origin = model.origin
        context.saveGState()
        defer { context.restoreGState() }
        context.scaleBy(x: z, y: z)
        context.translateBy(x: -origin.x, y: -origin.y)
        // Only what's in here needs drawing; maps can have thousands of rooms.
        let visible = CGRect(x: origin.x + dirtyRect.minX / z, y: origin.y + dirtyRect.minY / z, width: dirtyRect.width / z, height: dirtyRect.height / z)
            .insetBy(dx: -4 / z, dy: -4 / z)
        let thin = 1 / max(z, 0.5)

        // Exits under rooms. Exits to rooms elsewhere (or not mapped yet) get a short stub.
        let rooms = model.rooms
        func line(_ a: CGPoint, _ b: CGPoint) {
            guard max(a.x, b.x) >= visible.minX, min(a.x, b.x) <= visible.maxX, max(a.y, b.y) >= visible.minY, min(a.y, b.y) <= visible.maxY else { return }
            context.move(to: a)
            context.addLine(to: b)
        }
        for i in model.shown {
            let room = rooms[i]
            let from = room.center
            for exit in room.exits {
                if let target = exit.to.flatMap({ model.index[$0] }).map({ rooms[$0] }), target.area == room.area {
                    // Draw each two-way pair once.
                    if target.id < room.id, target.exits.contains(where: { $0.to == room.id }) { continue }
                    line(from, target.center)
                } else if let (dx, dy) = directionOffsets[exit.command.trimmingCharacters(in: .whitespaces).lowercased()] {
                    line(from, CGPoint(x: from.x + dx * model.gridX * 0.45, y: from.y + dy * model.gridY * 0.45))
                }
            }
        }
        NSColor.secondaryLabelColor.setStroke()
        context.setLineWidth(1.5 * thin)
        context.strokePath()

        // Names too small to read aren't worth laying out.
        let names = 9 * z >= 4
        let style = NSMutableParagraphStyle()
        style.alignment = .center
        style.lineBreakMode = .byWordWrapping
        let font = NSFont.systemFont(ofSize: 9)
        func attributes(_ color: NSColor) -> [NSAttributedString.Key: Any] {
            [.font: font, .paragraphStyle: style, .foregroundColor: color]
        }
        let plain = attributes(.labelColor)
        let onDark = attributes(.white)
        let onLight = attributes(.black)
        let options: NSString.DrawingOptions = [.usesLineFragmentOrigin, .truncatesLastVisibleLine]

        for i in model.shown {
            let room = rooms[i]
            let rect = room.rect
            guard rect.intersects(visible) else { continue }
            let isCurrent = room.id == model.current
            let isSelected = room.id == model.selected
            let radius = min(6, rect.width / 2, rect.height / 2)
            context.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
            (isCurrent ? NSColor.controlAccentColor : room.color ?? Self.fill).setFill()
            (isSelected ? NSColor.systemYellow : NSColor.secondaryLabelColor).setStroke()
            context.setLineWidth((isSelected ? 3 : 1) * thin)
            context.drawPath(using: .fillStroke)

            guard names, rect.width > 8, rect.height > 4 else { continue }
            let text = room.name as NSString
            let look = isCurrent ? onDark : room.color == nil ? plain : room.darkFill ? onDark : onLight
            let space = CGSize(width: rect.width - 8, height: rect.height - 4)
            let height = min(ceil(text.boundingRect(with: space, options: options, attributes: look).height), space.height)
            text.draw(with: CGRect(x: rect.minX + 4, y: rect.minY + (rect.height - height) / 2, width: space.width, height: height), options: options, attributes: look)
        }
    }

    private func drawHint() {
        let style = NSMutableParagraphStyle()
        style.alignment = .center
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: NSFont.systemFontSize),
            .paragraphStyle: style,
            .foregroundColor: NSColor.secondaryLabelColor,
        ]
        let text = Self.hint as NSString
        let width = max(bounds.width - 24, 40)
        let options: NSString.DrawingOptions = [.usesLineFragmentOrigin]
        let height = ceil(text.boundingRect(with: CGSize(width: width, height: .greatestFiniteMagnitude), options: options, attributes: attributes).height)
        text.draw(with: CGRect(x: (bounds.width - width) / 2, y: (bounds.height - height) / 2, width: width, height: height), options: options, attributes: attributes)
    }

    // MARK: Mouse

    private func point(_ event: NSEvent) -> CGPoint {
        convert(event.locationInWindow, from: nil)
    }

    override func mouseDown(with event: NSEvent) {
        guard let model else { return }
        // Overriding mouseDown takes Control-click away from the context menu; give it back.
        if event.modifierFlags.contains(.control) {
            if let menu = menu(for: event) { NSMenu.popUpContextMenu(menu, with: event, for: self) }
            return
        }
        let at = point(event)
        let spot = model.toMap(at)
        guard let room = model.room(at: spot) else {
            drag = .pan(origin: model.origin, from: at)
            return
        }
        model.select(room.id)
        if event.clickCount == 2 {
            drag = .none
            model.walk(to: room.id)
        } else {
            drag = .room(id: room.id, grab: CGPoint(x: spot.x - room.x, y: spot.y - room.y), from: at, moved: false)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let model else { return }
        let at = point(event)
        switch drag {
        case .none: break
        case .pan(let origin, let from):
            model.pan(to: CGPoint(x: origin.x - (at.x - from.x) / model.zoom, y: origin.y - (at.y - from.y) / model.zoom))
        case .room(let id, let grab, let from, let moved):
            // A click that wobbles shouldn't snap the room somewhere else.
            if !moved, hypot(at.x - from.x, at.y - from.y) < 3 { return }
            drag = .room(id: id, grab: grab, from: from, moved: true)
            let spot = model.toMap(at)
            var (x, y) = (spot.x - grab.x, spot.y - grab.y)
            // Snap to half grid cells unless Option (or Control, as elsewhere) is held, as in BeipMU.
            if event.modifierFlags.isDisjoint(with: [.option, .control]) {
                x = (x / (model.gridX / 2)).rounded() * (model.gridX / 2)
                y = (y / (model.gridY / 2)).rounded() * (model.gridY / 2)
            }
            model.drag(id, to: x, y)
        }
    }

    override func mouseUp(with event: NSEvent) {
        drag = .none
        model?.endDrag()
    }

    // The middle button pans wherever it starts.
    override func otherMouseDown(with event: NSEvent) {
        guard let model else { return }
        drag = .pan(origin: model.origin, from: point(event))
    }

    override func otherMouseDragged(with event: NSEvent) {
        mouseDragged(with: event)
    }

    override func otherMouseUp(with event: NSEvent) {
        drag = .none
    }

    override func scrollWheel(with event: NSEvent) {
        guard let model else { return }
        if event.hasPreciseScrollingDeltas {
            // A trackpad or Magic Mouse: the map moves with the fingers.
            model.pan(to: CGPoint(x: model.origin.x - event.scrollingDeltaX / model.zoom, y: model.origin.y - event.scrollingDeltaY / model.zoom))
        } else if event.scrollingDeltaY != 0 {
            // A wheel: zoom around the pointer.
            model.zoom(by: event.scrollingDeltaY > 0 ? 1.15 : 1 / 1.15, about: point(event))
        }
    }

    override func magnify(with event: NSEvent) {
        model?.zoom(by: 1 + event.magnification, about: point(event))
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let model else { return nil }
        let spot = model.toMap(point(event))
        let menu = NSMenu()
        if let room = model.room(at: spot) {
            let id = room.id
            model.select(id)
            menu.addItem(ActionItem("Walk Here") { model.walk(to: id) })
            menu.addItem(ActionItem("I’m Here") { model.setCurrent(id) })
            menu.addItem(ActionItem("Rename…") { model.rename(id) })
            menu.addItem(ActionItem("Delete Room") { model.delete(id) })
        } else {
            menu.addItem(ActionItem("Add Room Here") { model.addRoom(at: spot) })
        }
        return menu
    }
}

private struct MapCanvasView: NSViewRepresentable {
    let model: MapModel

    func makeNSView(context: Context) -> MapCanvas {
        let canvas = MapCanvas()
        canvas.model = model
        model.canvas = canvas
        canvas.describe()
        return canvas
    }

    func updateNSView(_ canvas: MapCanvas, context: Context) {
        if canvas.model !== model {
            canvas.model = model
            model.canvas = canvas
        }
    }
}

private struct MapPanelContent: View {
    @ObservedObject var model: MapModel
    let revision: Int
    @State private var query = ""

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                Picker("Area", selection: $model.area) {
                    ForEach(model.areas, id: \.self) { name in
                        Text(name.isEmpty ? "(no area)" : name).tag(name)
                    }
                }
                .labelsHidden()
                .frame(minWidth: 60, maxWidth: 150)
                .disabled(model.areas.isEmpty)
                .help("Area")

                TextField("Find room", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 60)
                    .onSubmit { model.search(query) }
                    .help("Find a room by name")

                Button { model.centerOnCurrent() } label: { Image(systemName: "scope") }
                    .help("Center on my room")
                    .accessibilityLabel("Center on my room")
                Toggle(isOn: $model.follow) { Image(systemName: "location.fill") }
                    .toggleStyle(.button)
                    .help("Follow my room as I move")
                    .accessibilityLabel("Follow my room as I move")
                Button { model.zoom(by: 1 / 1.25) } label: { Image(systemName: "minus.magnifyingglass") }
                    .help("Zoom out")
                    .accessibilityLabel("Zoom out")
                Button { model.zoom(by: 1.25) } label: { Image(systemName: "plus.magnifyingglass") }
                    .help("Zoom in")
                    .accessibilityLabel("Zoom in")
                Button { model.importMap() } label: { Image(systemName: "square.and.arrow.down") }
                    .help("Import a BeipMU map (.atlas)")
                    .accessibilityLabel("Import a BeipMU map")
            }
            .controlSize(.small)
            .padding(6)
            Divider()
            MapCanvasView(model: model)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onAppear { model.reloadIfNeeded() }
        .onChange(of: revision) { model.reloadIfNeeded() }
    }
}

struct MapPanel: View {
    @ObservedObject var session: Session

    var body: some View {
        MapPanelContent(model: MapModel.model(for: session), revision: session.mapRevision)
    }
}

// MARK: Beip.Tilemap

/// Shows a Beip.Tilemap map: tiles cut from one image, scaled to fit with sharp pixels.
private final class TilemapCanvas: NSView {
    var notice: (String) -> Void = { _ in }
    private var map = JSON.null
    private var sheet: CGImage?
    /// The URL being shown or waited for, and the one `sheet` came from.
    private var wantedURL = ""
    private var sheetURL = ""
    /// The whole map at 1:1, redrawn when the map changes.
    private var rendered: CGImage?
    /// Problems already reported, so a panel that's rebuilt doesn't repeat them.
    private static var reported: Set<String> = []

    override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel("Tile map")
    }

    required init?(coder: NSCoder) { fatalError("not in a nib") }

    /// Shows new map data, fetching the tile image if it changed.
    func update(_ new: JSON) {
        guard new != map else { return }
        map = new
        rendered = nil
        needsDisplay = true
        let url = new["tile_url"].string
        guard url != wantedURL else { return }
        wantedURL = url
        guard !url.isEmpty else { return }
        let failed = { [weak self] in
            guard let self, self.wantedURL == url else { return }
            self.complain("Couldn’t download the tiles from \(url).")
        }
        Images.shared.get(url, failed: failed) { [weak self] image in
            guard let self, self.wantedURL == url else { return }
            // The bitmap itself rather than the NSImage, whose size is in points and can differ.
            let bitmap = image.representations.compactMap { ($0 as? NSBitmapImageRep)?.cgImage }.max { $0.width < $1.width }
            guard let tiles = bitmap ?? image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
                return self.complain("Couldn’t read the tile image \(url).")
            }
            self.sheet = tiles
            self.sheetURL = url
            self.rendered = nil
            self.needsDisplay = true
        }
    }

    private func complain(_ problem: String) {
        if Self.reported.insert(problem).inserted { notice(problem) }
    }

    /// Draws every tile into one image at the tiles' own size.
    private func render() -> CGImage? {
        guard let sheet, sheetURL == wantedURL else { return nil }
        let (tw, th) = (map["tile_width"].int, map["tile_height"].int)
        let (width, height) = (map["width"].int, map["height"].int)
        guard tw > 0, th > 0, width > 0, height > 0 else { return nil }
        // The protocol allows 256 tiles of 256 pixels each way, which is more than a bitmap should be.
        guard width * tw <= 16384, height * th <= 16384 else {
            complain("The tile map is too big to show (\(width * tw) by \(height * th) pixels).")
            return nil
        }
        guard let context = CGContext(
            data: nil, width: width * tw, height: height * th, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        context.interpolationQuality = .none
        let perRow = max(sheet.width / tw, 1)
        var cut: [Int: CGImage] = [:]
        for (i, tile) in map["tiles"].array.prefix(width * height).enumerated() {
            let number = tile.int
            if cut[number] == nil {
                let (sx, sy) = ((number % perRow) * tw, (number / perRow) * th)
                // The tile image may not have this many tiles.
                guard number >= 0, sy + th <= sheet.height, sx + tw <= sheet.width else { continue }
                cut[number] = sheet.cropping(to: CGRect(x: sx, y: sy, width: tw, height: th))
            }
            guard let image = cut[number] else { continue }
            // Rows count from the top; a bitmap context's y counts from the bottom.
            context.draw(image, in: CGRect(x: (i % width) * tw, y: (height - 1 - i / width) * th, width: tw, height: th))
        }
        return context.makeImage()
    }

    override func draw(_ dirtyRect: NSRect) {
        if rendered == nil { rendered = render() }
        guard let rendered, let context = NSGraphicsContext.current?.cgContext else { return }
        let (w, h) = (CGFloat(rendered.width), CGFloat(rendered.height))
        let scale = min(bounds.width / w, bounds.height / h)
        guard scale > 0 else { return }
        // Keep pixel art sharp when enlarging; smooth it when shrinking, where nearest
        // neighbour would sparkle.
        context.interpolationQuality = scale >= 1 ? .none : .high
        context.draw(rendered, in: CGRect(x: (bounds.width - w * scale) / 2, y: (bounds.height - h * scale) / 2, width: w * scale, height: h * scale))
    }
}

struct TilemapPanel: NSViewRepresentable {
    let map: JSON
    let notice: (String) -> Void

    func makeNSView(context: Context) -> NSView {
        let canvas = TilemapCanvas()
        canvas.notice = notice
        canvas.update(map)
        return canvas
    }

    func updateNSView(_ view: NSView, context: Context) {
        guard let canvas = view as? TilemapCanvas else { return }
        canvas.notice = notice
        canvas.update(map)
    }
}
