import SwiftUI

/// The app's glyph (assets/icons/working-glyph.svg: six segments round a
/// dot) as the card's "working" mark, in the theme accent. viewBox 100 100 824 824.
struct WorkingGlyph: View {
    let accent: Color

    private static let data = "M744.09,174.01 A410,410 0 0 1 920.75,480.00 L734.71,480.00 A225,225 0 0 0 651.07,335.13 Z M920.75,544.00 A410,410 0 0 1 744.09,849.99 L651.07,688.87 A225,225 0 0 0 734.71,544.00 Z M688.66,881.99 A410,410 0 0 1 335.34,881.99 L428.36,720.87 A225,225 0 0 0 595.64,720.87 Z M279.91,849.99 A410,410 0 0 1 103.25,544.00 L289.29,544.00 A225,225 0 0 0 372.93,688.87 Z M103.25,480.00 A410,410 0 0 1 279.91,174.01 L372.93,335.13 A225,225 0 0 0 289.29,480.00 Z M335.34,142.01 A410,410 0 0 1 688.66,142.01 L595.64,303.13 A225,225 0 0 0 428.36,303.13 Z M392,512 a120,120 0 1 0 240,0 a120,120 0 1 0 -240,0 Z"

    var body: some View {
        Canvas { context, size in
            let s = min(size.width, size.height) / 824
            let path = SVGPathParser.path(from: Self.data)
                .applying(CGAffineTransform(scaleX: s, y: s).translatedBy(x: -100, y: -100))
            context.fill(path, with: .color(accent))
        }
    }
}
