import RustCodeKit
import SwiftUI

/// The guide to in-chat visualizations (`/visualize`, Help menu, + menu, the
/// empty chat): every widget type with how to use it, a live example drawn by
/// the real renderer, and a request to insert into the composer.
struct VisualizationGuideView: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.closePanel) private var close
    @State private var selection = VisualizationGuide.entries[0].type

    private var entry: VisualizationGuide.Entry {
        VisualizationGuide.entries.first { $0.type == selection } ?? VisualizationGuide.entries[0]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding([.horizontal, .top], 22)
                .padding(.bottom, 16)
            Divider()
            HStack(alignment: .top, spacing: 0) {
                typeList
                    .frame(width: 190)
                Divider()
                ScrollView {
                    detail.padding(20)
                }
                .id(entry.type)
            }
            Divider()
            footer
                .padding(.horizontal, 22)
                .padding(.vertical, 12)
        }
        .frame(width: 760, height: 600)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "chart.bar.doc.horizontal")
                .font(.system(size: 20, weight: .medium))
                .foregroundStyle(Color.brand)
                .frame(width: 30, height: 30)
                .background(Color.brand.opacity(0.12), in: .rect(cornerRadius: 9))
            VStack(alignment: .leading, spacing: 4) {
                Text("Charts, tables & trees in thread").font(.title3.weight(.semibold))
                Text(VisualizationGuide.overview)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Button("Done") { close() }
                .keyboardShortcut(.defaultAction)
        }
    }

    private var typeList: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(VisualizationGuide.entries) { item in
                Button { selection = item.type } label: {
                    Label(item.name, systemImage: item.systemImage)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(selection == item.type ? Color.brand.opacity(0.16) : .clear,
                                    in: .rect(cornerRadius: 8))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == item.type ? .isSelected : [])
            }
            Spacer(minLength: 0)
        }
        .padding(10)
    }

    private var detail: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(entry.name).font(.title3.weight(.semibold))
                Text(entry.purpose).foregroundStyle(.secondary)
                Label(entry.interaction, systemImage: "cursorarrow.click")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(.top, 2)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Try asking").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                HStack(alignment: .center, spacing: 12) {
                    Text("“\(entry.prompt)”")
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                    Button("Insert") { model.insertExamplePrompt(entry.prompt) }
                        .buttonStyle(PillButtonStyle(prominent: true))
                        .help("Put this request in the message field")
                }
                .padding(12)
                .background(.fill.quaternary, in: .rect(cornerRadius: 10))
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Example reply").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                MarkdownText(text: entry.sampleMarkdown, visualizations: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(VisualizationGuide.tips, id: \.self) { tip in
                Label(tip, systemImage: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}
