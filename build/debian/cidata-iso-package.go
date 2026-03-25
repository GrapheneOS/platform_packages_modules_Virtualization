// Copyright 2026 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package ferrochrome

import (
	"fmt"
	"path/filepath"

	"github.com/google/blueprint"

	"android/soong/android"
	"android/soong/cc"
)

//go:generate go run ../../../../../build/blueprint/gobtools/codegen

var pctx = android.NewPackageContext("android/soong/ferrochrome")

func init() {
	pctx.Import("android/soong/android")
	android.RegisterModuleType("cidata_iso_package", cidataIsoPackageFactory)
}

type packagingProperties struct {
	// iso label name
	Iso_label string

	// root dir. All subdirectories and files are implicitly added to deps.
	Root_dir string `android:"path"`

	// installation dir to put deps relative to root dir.
	Install_dir string

	// file to keep hash. The file should have {INSTANCE_ID} as a placeholder.
	Hash_file string
}

// Packaging iso for both arm64 and x86_64.
type cidataIsoPackage struct {
	android.ModuleBase
	android.PackagingBase
	blueprint.ModuleUsesIncrementalWalkDeps

	properties packagingProperties
}

// We need to implement IsNativeCoverageNeeded so that in coverage builds we don't get packaging
// conflicts with required deps that always use the coverage variant.
func (p *cidataIsoPackage) IsNativeCoverageNeeded(ctx cc.IsNativeCoverageNeededContext) bool {
	return ctx.DeviceConfig().NativeCoverageEnabled()
}

var _ cc.UseCoverage = (*cidataIsoPackage)(nil)

func cidataIsoPackageFactory() android.Module {
	module := &cidataIsoPackage{}
	android.InitPackageModule(module)
	android.InitAndroidArchModule(module, android.HostSupported, android.MultilibFirst)
	module.AddProperties(&module.properties)
	module.IgnoreMissingDependencies = true
	return module
}

type dependencyTag struct {
	blueprint.BaseDependencyTag
	android.InstallAlwaysNeededDependencyTag // to force installation of both "deps" and manually added dependencies
	android.PackagingItemAlwaysDepTag        // to force packaging of both "deps" and manually added dependencies
}

var cidataIsoPackageDependencyTag = dependencyTag{}

func (c *cidataIsoPackage) DepsMutator(ctx android.BottomUpMutatorContext) {
	c.AddDeps(ctx, cidataIsoPackageDependencyTag)

	ctx.AddHostToolDependencies("xorriso")
}

func (c *cidataIsoPackage) GenerateAndroidBuildActions(ctx android.ModuleContext) {
	sboxDir := android.PathForModuleOut(ctx, "sbox")
	sboxManifest := android.PathForModuleOut(ctx, "sbox.manifest")
	packageDir := sboxDir.Join(ctx, "staging_dir")
	installDir := packageDir.Join(ctx, c.properties.Install_dir)
	iso := sboxDir.Join(ctx, c.BaseModuleName()+".iso")

	builder := android.NewRuleBuilder(pctx, ctx).
		Sbox(sboxDir, sboxManifest).
		SandboxDisabled()
	builder.Command().BuiltTool("rm").Flag("-rf").Text(packageDir.String())
	builder.Command().Text("mkdir").Flag("-p").Text(packageDir.String())
	builder.Command().Text("mkdir").Flag("-p").Text(installDir.String())

	// Filter rust_proc_macros which are only needed on the build machine.
	rust_proc_macros := make(map[string]bool)
	ctx.WalkDepsProxy(func(child, parent android.ModuleProxy) bool {
		if ctx.OtherModuleType(child) == "rust_proc_macro" {
			name := ctx.OtherModuleName(child)
			rust_proc_macros[name] = true
		}
		return true
	})
	specs := c.GatherPackagingSpecsWithFilter(ctx, func(spec android.PackagingSpec) bool {
		return !rust_proc_macros[spec.Owner()]
	})
	c.CopySpecsToDir(ctx, builder, specs, installDir)

	rootDir := c.properties.Root_dir
	rootDirPath := android.PathForModuleSrc(ctx, rootDir)
	rootDirNodes := rootDirPath.String() + "/."

	// Soong's GlobFiles does not include hidden files/directories (starting with '.') by default.
	// We need to explicitly glob for them. Also, the build system does not allow multiple '**'
	// in a single pattern (e.g. '**/.*/**/*'), so we specify depths explicitly to cover
	// cases like 'root_files/home/droid/.config/weston.ini'.
	var rootDirAllFiles android.Paths
	rootDirAllFiles = append(rootDirAllFiles, ctx.GlobFiles(filepath.Join(rootDirPath.String(), "**/*"), nil)...)
	rootDirAllFiles = append(rootDirAllFiles, ctx.GlobFiles(filepath.Join(rootDirPath.String(), "**/.*/*"), nil)...)

	builder.Command().Text("cp").
		Flag("-R").
		Text(rootDirNodes).
		Implicits(rootDirAllFiles).
		Text(packageDir.String())

	builder.Command().Textf(
		"HASH=$(find . -type f -exec sha256sum {} + | sort | sha256sum | cut -d' ' -f1 | cut -c1-16); "+
			"sed -i 's/{INSTANCE_ID}/'${HASH}'/g' %s", packageDir.Join(ctx, c.properties.Hash_file))

	builder.Command().Text("chmod").Flag("-R").Flag("o=g").Text(packageDir.String())
	builder.Command().BuiltTool("xorriso").
		Flag("-as").
		Flag("mkisofs").
		Flag("-V").
		Text(c.properties.Iso_label).
		Flag("-J").
		Flag("-uid").
		Text("0").
		Flag("-gid").
		Text("0").
		Flag("-o").
		Output(iso).
		Flag("-R").
		Text(packageDir.String())
	builder.Build("cidata_iso", fmt.Sprintf("Creating iso for %s", c.BaseModuleName()))

	ctx.ModulePhonyFiles(iso)

	file_name := fmt.Sprintf("%s_%s.iso", ctx.ModuleName(), ctx.Arch().ArchType)
	ctx.DistForGoalWithFilename("ferrochrome_dist", iso, file_name)
}
