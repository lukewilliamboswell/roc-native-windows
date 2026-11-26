app [config] { pf: platform "https://github.com/lukewilliamboswell/roc-blueprint/releases/download/0.13.0/DBtzmMfrSgt6nCGVAcZYCVPxWbhpqw3MXpm6sdQzMn4v.tar.zst" }

import pf.Recipes

tooling = { compiler: "nightly-2026-11-26-890d6c8" }

config = [
	Name("roc-native-windows"),
	Systems(["aarch64-darwin", "aarch64-linux", "x86_64-linux", "x86_64-windows"]),
	Environment(
		"dev",
		[
			Programs([Fetch("roc-stable", Recipes.roc_nightly(tooling.compiler))]),
			RocPackages([
				"https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
				"https://github.com/roc-lang/http/releases/download/1.0.0/6ZUwqYhCS8PU9Mo6MF7oV82ET2o7KYb57CLKDq4cq4sS.tar.zst",
				"https://github.com/lukewilliamboswell/roc-zip/releases/download/0.3.0-rc4/7KtXa6LGYfVGZK5kDMtkHeumUJywvSR15YdQoRvfzxg1.tar.zst",
				"https://github.com/niclas-ahden/roc-crc32/releases/download/2.0.1/GVgp4MNmCBjdtJg8j2gBdsrdsKKyx4huzRz7ZtbRaYvJ.tar.zst",
				"https://github.com/lukewilliamboswell/roc-deflate/releases/download/0.6.0-rc1/2VEuxTntQvoRYzwwhGU22yHUiiC3cAgR1XyKajLw2Gpy.tar.zst",
				"https://github.com/lukewilliamboswell/weaver/releases/download/0.10.0/7CdYzjBAJmprmnK45oTpmH6BXt76ZASN2Ck5uZQEoWsK.tar.zst",
				"https://github.com/lukewilliamboswell/roc-fuzz/releases/download/0.7.0/HBigHQhVZWEF2EkLxpYK35u8P6UNsAmPYxpNFTxXK1Br.tar.zst",
				"https://github.com/lukewilliamboswell/roc-ansi/releases/download/0.13.3/BioPdu7WjVhTB2j1HaomKMi8htxsuwqwr79GbXY5nAUf.tar.zst",
				"https://github.com/roc-lang/path/releases/download/4.0.0/7YfABZPwJAXtLBY2vm8FqMyGAtNxncCJ65HdNKHFGNnE.tar.zst",
			]),
			Expect(["roc", "cargo", "rustc", "zig"]),
		],
	),
	Environment(
		"release",
		[
			Programs([Fetch("roc-stable", Recipes.roc_nightly(tooling.compiler))]),
			Expect(["roc"]),
		],
	),
	Shell("default", [Use("dev")]),
	Task("check", [Use("dev"), Run(["roc-stable", "scripts/build.roc", "--", "check"])]),
	Task("test-native", [Use("release"), Run(["roc-stable", "scripts/build.roc", "--", "test-native"])]),
	Task("build", [Use("dev"), Run(["roc-stable", "scripts/build.roc", "--", "build"])]),
	Task("bundle", [Use("dev"), Run(["roc-stable", "scripts/build.roc", "--", "bundle"])]),
	Task("packager", [Use("dev"), Run(["cargo", "build", "--locked", "--release", "--manifest-path", "packaging/Cargo.toml"])]),
	Task("release", [Use("release"), Run(["roc-stable", "scripts/release.roc", "--"])]),
	Task("serve", [Use("release"), Run(["roc-stable", "scripts/serve.roc", "--"])]),
]
