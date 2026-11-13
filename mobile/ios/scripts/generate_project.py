#!/usr/bin/env python3
"""Write mobile/ios/Notdot.xcodeproj deterministically from this file.

python3 mobile/ios/scripts/generate_project.py

Targets: Notdot (SwiftUI app), NotdotWidgets (Live Activity extension) and
NotdotTests (unit tests hosted in the app). The Rust core and its Swift
bindings are produced by scripts/build-core.sh in a build phase. IDs are
derived from names, so regenerating gives byte-identical output.
"""
from pathlib import Path
import hashlib

HERE = Path(__file__).resolve().parents[1]
PROJECT = HERE / 'Notdot.xcodeproj'


def uid(*parts):
    return hashlib.sha1('/'.join(parts).encode()).hexdigest()[:24].upper()


APP_SOURCES = ['Notdot/NotdotApp.swift', 'Notdot/ContentView.swift', 'Notdot/OrbView.swift', 'Notdot/OrbRenderer.swift',
               'Notdot/AppModel.swift', 'Notdot/LinkModel.swift', 'Notdot/AudioCapture.swift', 'Notdot/Intents.swift',
               'NotdotShared/SessionActivity.swift', 'build/generated/notdot_mobile_core.swift']
WIDGET_SOURCES = ['NotdotWidgets/NotdotWidgets.swift', 'NotdotShared/SessionActivity.swift']
TEST_SOURCES = ['NotdotTests/NotdotTests.swift']
TEST_RESOURCES = [('orb-golden', '../../crates/notdot-orb/fixtures/orb-golden'), ('wake-fixtures', '../../src-tauri/tests/fixtures')]
OTHER_FILES = ['Notdot/Info.plist', 'NotdotWidgets/Info.plist']

objects = []


def obj(id, body):
    objects.append((id, body))


def q(value):
    value = str(value)
    if value and all(c.isalnum() or c in '._/' for c in value) and not value[0].isdigit():
        return value
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'


def fmt(value, indent=2):
    pad = '\t' * indent
    if isinstance(value, dict):
        inner = ''.join(f'{pad}\t{k} = {fmt(v, indent + 1)};\n' for k, v in value.items())
        return '{\n' + inner + pad + '}'
    if isinstance(value, list):
        inner = ''.join(f'{pad}\t{fmt(v, indent + 1)},\n' for v in value)
        return '(\n' + inner + pad + ')'
    return q(value)


file_refs = {}
for path in sorted(set(APP_SOURCES + WIDGET_SOURCES + TEST_SOURCES + OTHER_FILES)):
    file_refs[path] = uid('ref', path)
    kind = 'sourcecode.swift' if path.endswith('.swift') else 'text.plist.xml'
    obj(file_refs[path], {'isa': 'PBXFileReference', 'lastKnownFileType': kind, 'path': path, 'sourceTree': '<group>'})
for name, path in TEST_RESOURCES:
    file_refs[name] = uid('ref', name)
    obj(file_refs[name], {'isa': 'PBXFileReference', 'lastKnownFileType': 'folder', 'name': name, 'path': path, 'sourceTree': '<group>'})

products = {'Notdot.app': ('wrapper.application', 'app'), 'NotdotWidgets.appex': ('wrapper.app-extension', 'widgets'),
            'NotdotTests.xctest': ('wrapper.cfbundle', 'tests')}
product_refs = {}
for name, (kind, key) in products.items():
    product_refs[key] = uid('product', name)
    obj(product_refs[key], {'isa': 'PBXFileReference', 'explicitFileType': kind, 'includeInIndex': '0', 'path': name, 'sourceTree': 'BUILT_PRODUCTS_DIR'})


def build_files(target, paths, phase):
    ids = []
    for path in paths:
        id = uid('build', target, phase, path)
        obj(id, {'isa': 'PBXBuildFile', 'fileRef': file_refs[path]})
        ids.append(id)
    return ids


def phase(target, isa, name, files, extra=None):
    id = uid('phase', target, name)
    body = {'isa': isa, 'buildActionMask': '2147483647', 'files': files, 'runOnlyForDeploymentPostprocessing': '0'}
    body.update(extra or {})
    obj(id, body)
    return id


common = {
    'IPHONEOS_DEPLOYMENT_TARGET': '18.0', 'SDKROOT': 'iphoneos', 'SWIFT_VERSION': '5.0', 'TARGETED_DEVICE_FAMILY': '1',
    'CODE_SIGN_STYLE': 'Automatic', 'CURRENT_PROJECT_VERSION': '1', 'MARKETING_VERSION': '0.1',
    'ENABLE_USER_SCRIPT_SANDBOXING': 'NO', 'SUPPORTED_PLATFORMS': 'iphoneos iphonesimulator',
}
core = {'SWIFT_INCLUDE_PATHS': '$(SRCROOT)/build/generated',
        'LIBRARY_SEARCH_PATHS': '$(SRCROOT)/build/lib/$(PLATFORM_NAME)'}


def config_list(owner, debug, release):
    ids = []
    for name, settings in (('Debug', debug), ('Release', release)):
        id = uid('config', owner, name)
        obj(id, {'isa': 'XCBuildConfiguration', 'buildSettings': settings, 'name': name})
        ids.append(id)
    list_id = uid('configlist', owner)
    obj(list_id, {'isa': 'XCConfigurationList', 'buildConfigurations': ids, 'defaultConfigurationIsVisible': '0',
                  'defaultConfigurationName': 'Release'})
    return list_id


def settings(extra, debug):
    merged = dict(common)
    merged.update(extra)
    if debug:
        merged['SWIFT_ACTIVE_COMPILATION_CONDITIONS'] = merged.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS', '') + ' DEBUG'
        merged['SWIFT_ACTIVE_COMPILATION_CONDITIONS'] = merged['SWIFT_ACTIVE_COMPILATION_CONDITIONS'].strip()
        merged['SWIFT_OPTIMIZATION_LEVEL'] = '-Onone'
        merged['DEBUG_INFORMATION_FORMAT'] = 'dwarf'
    return merged


app_settings = dict(core, **{
    'PRODUCT_NAME': 'Notdot', 'PRODUCT_BUNDLE_IDENTIFIER': 'dev.notdot.mobile.prototype',
    'INFOPLIST_FILE': 'Notdot/Info.plist', 'GENERATE_INFOPLIST_FILE': 'YES',
    'INFOPLIST_KEY_CFBundleDisplayName': 'notdot', 'INFOPLIST_KEY_UILaunchScreen_Generation': 'YES',
    'INFOPLIST_KEY_UISupportedInterfaceOrientations': 'UIInterfaceOrientationPortrait',
    'LD_RUNPATH_SEARCH_PATHS': '$(inherited) @executable_path/Frameworks',
    'OTHER_LDFLAGS': '-lnotdot_mobile_core -framework SherpaOnnxC -framework SystemConfiguration -framework Security -framework Network',
    'FRAMEWORK_SEARCH_PATHS': '$(SRCROOT)/build/lib/$(PLATFORM_NAME)',
    'SWIFT_ACTIVE_COMPILATION_CONDITIONS': 'NOTDOT_WAKE',
    'ASSETCATALOG_COMPILER_APPICON_NAME': '',
})
widget_settings = {
    'PRODUCT_NAME': 'NotdotWidgets', 'PRODUCT_BUNDLE_IDENTIFIER': 'dev.notdot.mobile.prototype.widgets',
    'INFOPLIST_FILE': 'NotdotWidgets/Info.plist', 'GENERATE_INFOPLIST_FILE': 'YES',
    'INFOPLIST_KEY_CFBundleDisplayName': 'notdot', 'SKIP_INSTALL': 'YES', 'APPLICATION_EXTENSION_API_ONLY': 'YES',
    'LD_RUNPATH_SEARCH_PATHS': '$(inherited) @executable_path/Frameworks @executable_path/../../Frameworks',
}
test_settings = dict(core, **{
    'PRODUCT_NAME': 'NotdotTests', 'PRODUCT_BUNDLE_IDENTIFIER': 'dev.notdot.mobile.prototype.tests',
    'GENERATE_INFOPLIST_FILE': 'YES', 'TEST_HOST': '$(BUILT_PRODUCTS_DIR)/Notdot.app/Notdot',
    'BUNDLE_LOADER': '$(TEST_HOST)',
})

# Widget target
widget_phases = [phase('widgets', 'PBXSourcesBuildPhase', 'Sources', build_files('widgets', WIDGET_SOURCES, 'sources')),
                 phase('widgets', 'PBXFrameworksBuildPhase', 'Frameworks', []),
                 phase('widgets', 'PBXResourcesBuildPhase', 'Resources', [])]
widget = uid('target', 'widgets')
obj(widget, {'isa': 'PBXNativeTarget', 'buildConfigurationList': config_list('widgets', settings(widget_settings, True), settings(widget_settings, False)),
             'buildPhases': widget_phases, 'buildRules': [], 'dependencies': [], 'name': 'NotdotWidgets',
             'productName': 'NotdotWidgets', 'productReference': product_refs['widgets'],
             'productType': 'com.apple.product-type.app-extension'})

# App target
embed = uid('build', 'app', 'embed', 'widgets')
obj(embed, {'isa': 'PBXBuildFile', 'fileRef': product_refs['widgets'], 'settings': {'ATTRIBUTES': ['RemoveHeadersOnCopy']}})
script = uid('phase', 'app', 'rust')
obj(script, {'isa': 'PBXShellScriptBuildPhase', 'buildActionMask': '2147483647', 'files': [], 'inputPaths': [],
             'name': 'Build Rust core', 'outputPaths': [], 'runOnlyForDeploymentPostprocessing': '0',
             'shellPath': '/bin/sh', 'shellScript': '"$SRCROOT/scripts/build-core.sh" "$PLATFORM_NAME" "$CONFIGURATION"\n',
             'alwaysOutOfDate': '1'})
embed_native = uid('phase', 'app', 'embed-native')
obj(embed_native, {'isa': 'PBXShellScriptBuildPhase', 'buildActionMask': '2147483647', 'files': [], 'inputPaths': [],
                   'name': 'Embed wake library and models', 'outputPaths': [], 'runOnlyForDeploymentPostprocessing': '0',
                   'shellPath': '/bin/sh', 'shellScript': '"$SRCROOT/scripts/embed-native.sh"\n', 'alwaysOutOfDate': '1'})
proxy = uid('proxy', 'widgets')
dependency = uid('dependency', 'widgets')
app = uid('target', 'app')
project = uid('project')
obj(proxy, {'isa': 'PBXContainerItemProxy', 'containerPortal': project, 'proxyType': '1', 'remoteGlobalIDString': widget,
            'remoteInfo': 'NotdotWidgets'})
obj(dependency, {'isa': 'PBXTargetDependency', 'target': widget, 'targetProxy': proxy})
app_phases = [script,
              phase('app', 'PBXSourcesBuildPhase', 'Sources', build_files('app', APP_SOURCES, 'sources')),
              phase('app', 'PBXFrameworksBuildPhase', 'Frameworks', []),
              phase('app', 'PBXResourcesBuildPhase', 'Resources', []),
              phase('app', 'PBXCopyFilesBuildPhase', 'Embed Foundation Extensions', [embed],
                    {'dstPath': '', 'dstSubfolderSpec': '13', 'name': 'Embed Foundation Extensions'}),
              embed_native]
obj(app, {'isa': 'PBXNativeTarget', 'buildConfigurationList': config_list('app', settings(app_settings, True), settings(app_settings, False)),
          'buildPhases': app_phases, 'buildRules': [], 'dependencies': [dependency], 'name': 'Notdot',
          'productName': 'Notdot', 'productReference': product_refs['app'],
          'productType': 'com.apple.product-type.application'})

# Test target
test_proxy = uid('proxy', 'app')
test_dependency = uid('dependency', 'app')
obj(test_proxy, {'isa': 'PBXContainerItemProxy', 'containerPortal': project, 'proxyType': '1', 'remoteGlobalIDString': app,
                 'remoteInfo': 'Notdot'})
obj(test_dependency, {'isa': 'PBXTargetDependency', 'target': app, 'targetProxy': test_proxy})
resource_files = []
for name, _ in TEST_RESOURCES:
    id = uid('build', 'tests', 'resources', name)
    obj(id, {'isa': 'PBXBuildFile', 'fileRef': file_refs[name]})
    resource_files.append(id)
tests = uid('target', 'tests')
obj(tests, {'isa': 'PBXNativeTarget', 'buildConfigurationList': config_list('tests', settings(test_settings, True), settings(test_settings, False)),
            'buildPhases': [phase('tests', 'PBXSourcesBuildPhase', 'Sources', build_files('tests', TEST_SOURCES, 'sources')),
                            phase('tests', 'PBXFrameworksBuildPhase', 'Frameworks', []),
                            phase('tests', 'PBXResourcesBuildPhase', 'Resources', resource_files)],
            'buildRules': [], 'dependencies': [test_dependency], 'name': 'NotdotTests', 'productName': 'NotdotTests',
            'productReference': product_refs['tests'], 'productType': 'com.apple.product-type.bundle.unit-test'})

# Groups
products_group = uid('group', 'Products')
obj(products_group, {'isa': 'PBXGroup', 'children': list(product_refs.values()), 'name': 'Products', 'sourceTree': '<group>'})
main_group = uid('group', 'main')
children = [file_refs[p] for p in sorted(file_refs) if p not in dict(TEST_RESOURCES)] + [file_refs[n] for n, _ in TEST_RESOURCES]
obj(main_group, {'isa': 'PBXGroup', 'children': children + [products_group], 'sourceTree': '<group>'})

project_settings = {'ALWAYS_SEARCH_USER_PATHS': 'NO', 'CLANG_ENABLE_MODULES': 'YES', 'ENABLE_STRICT_OBJC_MSGSEND': 'YES',
                    'IPHONEOS_DEPLOYMENT_TARGET': '18.0', 'SDKROOT': 'iphoneos'}
obj(project, {'isa': 'PBXProject', 'attributes': {'BuildIndependentTargetsInParallel': '1', 'LastUpgradeCheck': '2700'},
              'buildConfigurationList': config_list('project', dict(project_settings, ONLY_ACTIVE_ARCH='YES', ENABLE_TESTABILITY='YES'), project_settings),
              'compatibilityVersion': 'Xcode 14.0', 'developmentRegion': 'en', 'hasScannedForEncodings': '0',
              'knownRegions': ['en', 'Base'], 'mainGroup': main_group, 'productRefGroup': products_group,
              'projectDirPath': '', 'projectRoot': '', 'targets': [app, widget, tests]})

lines = ['// !$*UTF8*$!', '{', '\tarchiveVersion = 1;', '\tclasses = {', '\t};', '\tobjectVersion = 60;', '\tobjects = {']
for id, body in sorted(objects):
    lines.append(f'\t\t{id} = {fmt(body, 2)};')
lines += ['\t};', f'\trootObject = {project};', '}', '']
PROJECT.mkdir(exist_ok=True)
(PROJECT / 'project.pbxproj').write_text('\n'.join(lines))

scheme = f'''<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion = "2700" version = "1.7">
   <BuildAction parallelizeBuildables = "YES" buildImplicitDependencies = "YES">
      <BuildActionEntries>
         <BuildActionEntry buildForTesting = "YES" buildForRunning = "YES" buildForProfiling = "YES" buildForArchiving = "YES" buildForAnalyzing = "YES">
            <BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{app}" BuildableName = "Notdot.app" BlueprintName = "Notdot" ReferencedContainer = "container:Notdot.xcodeproj">
            </BuildableReference>
         </BuildActionEntry>
      </BuildActionEntries>
   </BuildAction>
   <TestAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" shouldUseLaunchSchemeArgsEnv = "NO">
      <CommandLineArguments>
         <CommandLineArgument argument = "-NotdotSilentTest YES" isEnabled = "YES">
         </CommandLineArgument>
      </CommandLineArguments>
      <EnvironmentVariables>
         <EnvironmentVariable key = "NOTDOT_SILENT" value = "1" isEnabled = "YES">
         </EnvironmentVariable>
      </EnvironmentVariables>
      <Testables>
         <TestableReference skipped = "NO">
            <BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{tests}" BuildableName = "NotdotTests.xctest" BlueprintName = "NotdotTests" ReferencedContainer = "container:Notdot.xcodeproj">
            </BuildableReference>
         </TestableReference>
      </Testables>
   </TestAction>
   <LaunchAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" launchStyle = "0" useCustomWorkingDirectory = "NO" ignoresPersistentStateOnLaunch = "NO" debugDocumentVersioning = "YES" debugServiceExtension = "internal" allowLocationSimulation = "YES">
      <BuildableProductRunnable runnableDebuggingMode = "0">
         <BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{app}" BuildableName = "Notdot.app" BlueprintName = "Notdot" ReferencedContainer = "container:Notdot.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </LaunchAction>
   <ArchiveAction buildConfiguration = "Release" revealArchiveInOrganizer = "YES">
   </ArchiveAction>
</Scheme>
'''
schemes = PROJECT / 'xcshareddata' / 'xcschemes'
schemes.mkdir(parents=True, exist_ok=True)
(schemes / 'Notdot.xcscheme').write_text(scheme)
print('wrote', PROJECT.relative_to(HERE.parents[1]))
