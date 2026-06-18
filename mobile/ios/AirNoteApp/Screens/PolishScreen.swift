import AirNoteShared
import SwiftUI
import UIKit

/// "Polish a written message" — takes text the user has already typed (e.g. a
/// rough message) and rewrites it clean and clear via the server's
/// /v1/runtime/message-polish endpoint. Distinct from voice dictation.
struct PolishScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var input = ""
    @State private var output = ""
    @State private var isPolishing = false
    @State private var errorMessage: String?
    @FocusState private var inputFocused: Bool

    private var trimmed: String { input.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Paste or type a message and AirNote rewrites it clean and clear. This is for text you've already typed — different from voice dictation.")
                        .font(.footnote)
                        .foregroundStyle(AirNoteDesign.muted)

                    VStack(alignment: .leading, spacing: 8) {
                        AirNoteSectionLabel(text: "Your message")
                        ZStack(alignment: .topLeading) {
                            TextEditor(text: $input)
                                .frame(minHeight: 150)
                                .focused($inputFocused)
                                .font(.body)
                                .scrollContentBackground(.hidden)
                                .padding(10)
                                .background(AirNoteDesign.surface.opacity(0.92), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(AirNoteDesign.border, lineWidth: 1))
                            if input.isEmpty {
                                Text("Type or paste here…")
                                    .font(.body)
                                    .foregroundStyle(AirNoteDesign.muted)
                                    .padding(.horizontal, 15)
                                    .padding(.vertical, 18)
                                    .allowsHitTesting(false)
                            }
                        }
                    }

                    Button {
                        Task { await polish() }
                    } label: {
                        HStack(spacing: 8) {
                            if isPolishing {
                                ProgressView().tint(.white)
                            } else {
                                Image(systemName: "wand.and.stars")
                            }
                            Text(isPolishing ? "Polishing…" : "Polish")
                                .font(.headline)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                        .background(AirNoteDesign.accent, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .foregroundStyle(.white)
                    }
                    .buttonStyle(.plain)
                    .disabled(isPolishing || trimmed.isEmpty)
                    .opacity(trimmed.isEmpty ? 0.5 : 1)

                    if let errorMessage {
                        Text(errorMessage)
                            .font(.footnote)
                            .foregroundStyle(AirNoteDesign.warning)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }

                    if !output.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                AirNoteSectionLabel(text: "Polished")
                                Spacer()
                                Button {
                                    UIPasteboard.general.string = output
                                } label: {
                                    Label("Copy", systemImage: "doc.on.doc")
                                        .font(.caption.weight(.semibold))
                                }
                                .buttonStyle(.plain)
                                .foregroundStyle(AirNoteDesign.accent)
                            }
                            Text(output)
                                .font(.body)
                                .foregroundStyle(AirNoteDesign.foreground)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                                .background(AirNoteDesign.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                                .textSelection(.enabled)
                        }
                    }
                }
                .padding(16)
            }
        }
        .navigationTitle("Polish")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func polish() async {
        inputFocused = false
        errorMessage = nil
        let text = trimmed
        guard !text.isEmpty else { return }
        isPolishing = true
        defer { isPolishing = false }
        do {
            output = try await env.gateway.messagePolish(text: text)
        } catch {
            output = ""
            errorMessage = (error as? GatewayError)?.userMessage ?? "Couldn't polish that. Please try again."
        }
    }
}
