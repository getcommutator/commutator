VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TARGET  := target/release
APP     := target/Commutator.app
DEV_APP := target/Commutator-dev.app
ICONSET := target/commutator.iconset
ICNS    := target/commutator.icns
SIGN_ID ?= Developer ID Application: Egor Kolesnikov (Z7PK4B5KU2)
NOTARY_PROFILE ?= commutator
CODESIGN = codesign --force --options runtime --timestamp --entitlements dist/macos/entitlements.plist --sign "$(SIGN_ID)"

.PHONY: build app run dmg notarize release clean

# $(call bundle,<app>,<binaries dir>,<codesign command>)
define bundle
	rm -rf $(1)
	mkdir -p $(1)/Contents/MacOS $(1)/Contents/Resources
	sed 's/__VERSION__/$(VERSION)/g' dist/macos/Info.plist > $(1)/Contents/Info.plist
	cp $(2)/commutator $(1)/Contents/MacOS/commutator
	cp $(2)/commutator-engine "$(1)/Contents/MacOS/Commutator Engine"
	cp $(ICNS) $(1)/Contents/Resources/commutator.icns
	$(3) "$(1)/Contents/MacOS/Commutator Engine"
	$(3) $(1)
endef

build:
	cargo build --release -p commutator --bins

app: build $(ICNS)
	$(call bundle,$(APP),$(TARGET),$(CODESIGN))
	codesign --verify --strict --verbose=2 $(APP)
	@echo "→ $(APP)"

$(ICNS): assets/icon.png
	rm -rf $(ICONSET) && mkdir -p $(ICONSET)
	for s in 16 32 128 256 512; do \
		sips -s format png -z $$s $$s $< --out $(ICONSET)/icon_$${s}x$${s}.png >/dev/null; \
		sips -s format png -z $$((s*2)) $$((s*2)) $< --out $(ICONSET)/icon_$${s}x$${s}@2x.png >/dev/null; \
	done
	iconutil -c icns $(ICONSET) -o $@

# For testing: the dev profile (incremental, our crates unoptimized), signed
# ad hoc. Distribution builds go through `app`.
run: $(ICNS)
	cargo build -p commutator --bins
	$(call bundle,$(DEV_APP),target/debug,codesign --force --entitlements dist/macos/entitlements.plist --sign -)
	-pkill -f '[C]ommutator-dev.app/Contents/MacOS/commutator'
	exec $(DEV_APP)/Contents/MacOS/commutator

dmg: app
	rm -rf target/dmg && mkdir -p target/dmg && ditto $(APP) target/dmg/$(notdir $(APP))
	ln -s /Applications target/dmg/Applications  # drag-to-install target
	hdiutil create -volname Commutator -srcfolder target/dmg -ov -format UDZO target/Commutator-$(VERSION).dmg

# One-time: xcrun notarytool store-credentials $(NOTARY_PROFILE) --apple-id <id> --team-id Z7PK4B5KU2
notarize: dmg
	codesign --force --timestamp --sign "$(SIGN_ID)" target/Commutator-$(VERSION).dmg
	xcrun notarytool submit target/Commutator-$(VERSION).dmg --keychain-profile $(NOTARY_PROFILE) --wait
	xcrun stapler staple target/Commutator-$(VERSION).dmg
	xcrun stapler staple $(APP)

# What the app's updater reads (crates/update): the app tarball and a
# manifest.json with its sha256, on a GitHub release tagged v<version>.
PAYLOAD := commutator-$(VERSION)-macos-$(shell uname -m)-app.tar.gz

release: notarize
	COPYFILE_DISABLE=1 tar -czf target/$(PAYLOAD) -C target Commutator.app
	printf '{"version":"%s","files":{"%s":{"sha256":"%s"}}}\n' $(VERSION) $(PAYLOAD) \
		$$(shasum -a 256 target/$(PAYLOAD) | cut -d' ' -f1) > target/manifest.json
	gh release create v$(VERSION) target/$(PAYLOAD) target/manifest.json target/Commutator-$(VERSION).dmg \
		--title v$(VERSION) --generate-notes

clean:
	rm -rf $(APP) $(DEV_APP) $(ICONSET) $(ICNS)
