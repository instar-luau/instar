#[path = "support/roblox.rs"]
mod support;

use instar_core::{
    analysis::{self, Options},
    project::resolution::Resolver,
    source::SourceStore,
};
use std::{error::Error, fs, path::Path};

type Result = std::result::Result<(), Box<dyn Error>>;

fn checker()
-> impl FnMut(&Path, &str, bool) -> std::result::Result<analysis::Report, Box<dyn Error>> {
    let mut session = analysis::Session::default();

    move |root, source, old_solver| {
        if root.join("instar.toml").exists() {
            support::configure(root)?;
        }

        let path = root.join("main.luau");
        fs::write(&path, source)?;

        Ok(session.analyze(
            &mut SourceStore::default(),
            &[path],
            &Options {
                old_solver,
                ..Default::default()
            },
        )?)
    }
}

fn valid(report: &analysis::Report) {
    assert!(
        !report.has_errors(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.message)
            .collect::<Vec<_>>()
    );
}

#[test]
fn cached_environment_is_explicit_and_contains_datatypes_and_documented_signatures() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let source = "--!strict\nlocal part: Part = Instance.new('Part')\nlocal position: Vector3 = Vector3.new(1, 2, 3) + Vector3.one\npart.Position = position\npart.Material = Enum.Material.Plastic\nlocal workspace: Workspace = game:GetService('Workspace')\nlocal result: RaycastResult? = workspace:Raycast(position, Vector3.yAxis)\nlocal connection: RBXScriptConnection = part.Touched:Once(function(other: BasePart) print(other.Name) end)\nconnection:Disconnect()\nreturn part, result";

    for old_solver in [false, true] {
        assert!(check(root, source, old_solver)?.has_errors());
    }

    fs::write(root.join("instar.toml"), "[roblox]")?;

    for old_solver in [false, true] {
        let report = check(root, source, old_solver)?;
        valid(&report);

        assert!(
            report.documentation[&root.join("main.luau")]
                .contains_key("@roblox/globaltype/Instance.Name")
        );

        assert!(
            check(
                root,
                "--!strict\nlocal value: number = Vector3.new()\nreturn value",
                old_solver
            )?
            .has_errors()
        );
    }

    Ok(())
}

#[test]
fn roblox_globals_constructors_signals_and_equality() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[roblox]")?;
    let source = "--!strict\nlocal font: Font = Font.new('rbxasset://fonts/families/SourceSansPro.json')\nlocal family: string = font.Family\nlocal other: Font = Font.fromEnum(Enum.Font.SourceSans)\nlocal dimension: UDim = UDim.new()\nlocal storage: buffer = buffer.create(1)\nwarn(storage)\nlocal button = Instance.new('TextButton')\nlocal connection: RBXScriptConnection = button.MouseButton1Click:Connect(function() warn(family) end)\nlocal signal: RBXScriptSignal<()> = button.MouseButton1Click\nlocal item: EnumItem = Enum.KeyCode.Space\nreturn function(name: string, instance: Instance, humanoid: Humanoid)\nlocal group: Enum = Enum[name]\nreturn group == Enum.KeyCode, item.EnumType ~= Enum.KeyCode, instance ~= humanoid, font, other, dimension, signal, connection\nend";

    for old_solver in [false, true] {
        valid(&check(root, source, old_solver)?);

        for invalid in [
            "--!strict\nlocal value: Font = Enum.Font.SourceSans\nreturn value",
            "--!strict\nlocal value: Enum.Font = Font.new('sample')\nreturn value",
            "--!strict\nlocal value: Buffer = buffer.create(1)\nreturn value",
            "--!strict\nInstance.new('TextButton').MouseButton1Click:Connect(function(value: number) print(value) end)",
            "--!strict\nreturn UDim.new('sample')",
        ] {
            assert!(check(root, invalid, old_solver)?.has_errors());
        }
    }

    Ok(())
}

#[test]
fn datatype_collections_variadics_and_opaque_references_preserve_types() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[roblox]")?;

    for old_solver in [false, true] {
        for source in [
            "local parameters = CatalogSearchParams.new()\nlocal assets: {Enum.AvatarAssetType} = parameters.AssetTypes\nlocal bundles: {Enum.BundleType} = parameters.BundleTypes\nreturn assets, bundles",
            "local points: {ColorSequenceKeypoint} = ColorSequence.new(Color3.new()).Keypoints\nlocal color: Color3 = points[1].Value\nreturn color",
            "local points: {NumberSequenceKeypoint} = NumberSequence.new(1).Keypoints\nlocal value: number = points[1].Value\nreturn value",
            "local parameters = RaycastParams.new()\nparameters.ExcludeInstances = nil\nlocal instances: {Instance} = {Instance.new('Part')}\nparameters.ExcludeInstances = instances\nlocal selected: {Instance}? = parameters.ExcludeInstances\nreturn selected",
            "local empty: SecurityCapabilities = SecurityCapabilities.new()\nlocal capabilities: SecurityCapabilities = SecurityCapabilities.new(Enum.SecurityCapability.Players, Enum.SecurityCapability.Animation)\nreturn empty, capabilities",
            "local evaluator: ClipEvaluator = game:GetService('AnimationClipProvider'):GetClipEvaluatorAsync('sample')\nreturn evaluator",
            "local players: {Player} = game:GetService('Players'):GetPlayers()\nreturn players",
            "local event = Instance.new('RemoteEvent')\nevent.OnServerEvent:Connect(function(player: Player, count: number, message: string) print(player, count, message) end)\nevent.OnClientEvent:Connect(function(count: number, message: string) print(count, message) end)",
        ] {
            valid(&check(root, &format!("--!strict\n{source}"), old_solver)?);
        }

        for source in [
            "local value: number = CatalogSearchParams.new().AssetTypes[1]\nreturn value",
            "local value: number = ColorSequence.new(Color3.new()).Keypoints[1].Value\nreturn value",
            "local value: Color3 = NumberSequence.new(1).Keypoints[1].Value\nreturn value",
            "local parameters = RaycastParams.new()\nparameters.ExcludeInstances = {1}",
            "local value: number = Instance.new('Folder'):GetChildren()[1]\nreturn value",
            "return SecurityCapabilities.new(1)",
            "local value: number = game:GetService('AnimationClipProvider'):GetClipEvaluatorAsync('sample')\nreturn value",
            "return game:GetService('AnimationClipProvider'):GetClipEvaluatorAsync('sample').Missing",
        ] {
            assert!(
                check(root, &format!("--!strict\n{source}"), old_solver)?.has_errors(),
                "{source}"
            );
        }
    }

    Ok(())
}

#[test]
fn datatype_signatures_preserve_operators_packs_and_nullable_seats() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[roblox]")?;

    for old_solver in [false, true] {
        for (name, value) in [
            ("Vector2", "Vector2.new(1, 2)"),
            ("Vector3", "Vector3.new(1, 2, 3)"),
            ("Vector2int16", "Vector2int16.new(1, 2)"),
            ("Vector3int16", "Vector3int16.new(1, 2, 3)"),
            ("UDim", "UDim.new(1, 2)"),
            ("UDim2", "UDim2.new(1, 2, 3, 4)"),
        ] {
            valid(&check(
                root,
                &format!("--!strict\nlocal value: {name} = -{value}\nreturn value"),
                old_solver,
            )?);
        }

        for (method, name, value) in [
            ("ToWorldSpace", "CFrame", "CFrame.new()"),
            ("ToObjectSpace", "CFrame", "CFrame.new()"),
            ("PointToWorldSpace", "Vector3", "Vector3.zero"),
            ("PointToObjectSpace", "Vector3", "Vector3.zero"),
            ("VectorToWorldSpace", "Vector3", "Vector3.zero"),
            ("VectorToObjectSpace", "Vector3", "Vector3.zero"),
        ] {
            valid(&check(
                root,
                &format!(
                    "--!strict\nlocal first: {name}, second: {name} = CFrame.new():{method}({value}, {value})\nreturn first, second"
                ),
                old_solver,
            )?);

            assert!(
                check(
                    root,
                    &format!("--!strict\nreturn CFrame.new():{method}(1)"),
                    old_solver
                )?
                .has_errors()
            );

            assert!(check(root, &format!("--!strict\nlocal value: number = CFrame.new():{method}({value})\nreturn value"), old_solver)?.has_errors());
        }

        valid(&check(
            root,
            "--!strict\nlocal frame = CFrame.new()\nlocal inverse: CFrame = frame:Inverse()\nlocal interpolated: CFrame = frame:Lerp(inverse, 0.5)\nlocal normalized: CFrame = frame:Orthonormalize()\nlocal equal: boolean = frame:FuzzyEq(inverse)\nlocal axis: Vector3, angle: number = frame:ToAxisAngle()\nlocal position: Vector3 = frame * Vector3.zero\nlocal translated: CFrame = frame + Vector3.zero - Vector3.zero\nlocal composed: CFrame = frame * frame\nreturn interpolated, normalized, equal, axis, angle, position, translated, composed",
            old_solver,
        )?);

        let numbers = ["number"; 12].join(", ");

        valid(&check(
            root,
            &format!(
                "--!strict\nlocal components: (CFrame) -> ({numbers}) = CFrame.new().GetComponents\nreturn components"
            ),
            old_solver,
        )?);

        assert!(check(root, "--!strict\nlocal components: (CFrame) -> string = CFrame.new().GetComponents\nreturn components", old_solver)?.has_errors());

        valid(&check(
            root,
            "--!strict\nreturn function(humanoid: Humanoid): (Seat | VehicleSeat)? return humanoid.SeatPart end",
            old_solver,
        )?);

        assert!(
            check(
                root,
                "--!strict\nlocal value: number = -Vector3.zero\nreturn value",
                old_solver
            )?
            .has_errors()
        );
    }

    Ok(())
}

#[test]
fn mappings_preserve_imports_properties_and_script_kinds() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[roblox]\nsourcemap = 'sourcemap.json'",
    )?;

    let sourcemap = r#"{"name":"Game","className":"DataModel","children":[{"name":"Storage","className":"ReplicatedStorage","children":[{"name":"Main","className":"ModuleScript","filePaths":["main.luau"]},{"name":"Entry","className":"ModuleScript","filePaths":["entry.luau"]},{"name":"Name","className":"ModuleScript","filePaths":["name.luau"]}]}]}"#;

    fs::write(root.join("entry.luau"), "--!strict\nreturn {Value = 1}")?;
    fs::write(root.join("name.luau"), "return 2")?;
    fs::write(root.join("worker.server.luau"), "return 3")?;

    for old_solver in [false, true] {
        fs::write(root.join("sourcemap.json"), sourcemap)?;

        for expression in [
            "script.Parent.Entry",
            "game:GetService('ReplicatedStorage').Entry",
            "script.Parent:WaitForChild('Entry')",
            "script.Parent:FindFirstChild('Entry')",
        ] {
            valid(&check(
                root,
                &format!(
                    "--!strict\nlocal value: number = require({expression}).Value\nlocal name: string = script.Parent.Name\nlocal other: number = require(script.Parent:WaitForChild('Name'))\nreturn value, name, other"
                ),
                old_solver,
            )?);

            assert!(
                check(
                    root,
                    &format!(
                        "--!strict\nlocal value: string = require({expression}).Value\nreturn value"
                    ),
                    old_solver
                )?
                .has_errors(),
                "solver={old_solver} expression={expression}"
            );
        }

        assert!(
            check(
                root,
                "--!strict\nreturn require('./worker.server')",
                old_solver
            )?
            .has_errors()
        );

        valid(&check(
            root,
            &format!("return '{}'", "sample".repeat(1024)),
            old_solver,
        )?);

        fs::write(
            root.join("sourcemap.json"),
            r#"{"name":"Game","className":"DataModel","children":[{"name":"Replacement","className":"Folder"}]}"#,
        )?;

        valid(&check(
            root,
            "--!strict\nlocal replacement: Folder = game.Replacement\nreturn replacement",
            old_solver,
        )?);

        assert!(check(root, "--!strict\nreturn game.Storage", old_solver)?.has_errors());
    }

    Ok(())
}

#[test]
fn security_levels_filter_inaccessible_members() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    for old_solver in [false, true] {
        fs::write(root.join("instar.toml"), "[roblox]\nlevel = 'None'")?;

        for source in [
            "--!strict\nreturn Instance.new('Part').RobloxLocked",
            "--!strict\nInstance.new('Part').RobloxLocked = true",
        ] {
            assert!(
                check(root, source, old_solver)?.has_errors(),
                "solver={old_solver} source={source}"
            );
        }

        valid(&check(
            root,
            "--!strict\nInstance.new('BindableFunction').OnInvoke = function() return 1 end",
            old_solver,
        )?);

        valid(&check(
            root,
            "--!strict\nlocal height: number = workspace.FallenPartsDestroyHeight\nreturn height",
            old_solver,
        )?);

        fs::write(
            root.join("instar.toml"),
            "[roblox]\nlevel = 'LocalUserSecurity'",
        )?;

        assert!(
            check(
                root,
                "--!strict\nreturn Instance.new('Part').RobloxLocked",
                old_solver
            )?
            .has_errors()
        );

        fs::write(root.join("instar.toml"), "[roblox]")?;

        valid(&check(
            root,
            "--!strict\nlocal value: boolean = Instance.new('Part').RobloxLocked\nreturn value",
            old_solver,
        )?);

        valid(&check(
            root,
            "--!strict\nworkspace.FallenPartsDestroyHeight = -500",
            old_solver,
        )?);

        fs::write(
            root.join("instar.toml"),
            "[roblox]\nlevel = 'PluginSecurity'",
        )?;

        valid(&check(
            root,
            "--!strict\nInstance.new('Part').RobloxLocked = true",
            old_solver,
        )?);

        assert!(
            check(
                root,
                "--!strict\nreturn Instance.new('Part').UniqueId",
                old_solver
            )?
            .has_errors()
        );

        fs::write(
            root.join("instar.toml"),
            "[roblox]\nlevel = 'RobloxScriptSecurity'",
        )?;

        valid(&check(
            root,
            "--!strict\nreturn Instance.new('Part').UniqueId",
            old_solver,
        )?);

    }

    Ok(())
}

#[test]
fn levels_inherit_and_validate() -> Result {
    let mut check = checker();

    assert!(
        instar_core::configuration::InstarConfig::parse("[roblox]\nlevel = 'Unknown'").is_err()
    );

    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    fs::write(root.join("instar.toml"), "[roblox]\nlevel = 'None'")?;
    fs::write(child.join("instar.toml"), "[roblox]")?;
    let source = "--!strict\nInstance.new('Part').RobloxLocked = true";

    for old_solver in [false, true] {
        fs::write(child.join("instar.toml"), "[roblox]")?;
        assert!(check(&child, source, old_solver)?.has_errors());

        fs::write(
            child.join("instar.toml"),
            "[roblox]\nlevel = 'PluginSecurity'",
        )?;

        valid(&check(&child, source, old_solver)?);
    }

    Ok(())
}

#[test]
fn predicates_and_class_lookups_use_cached_metadata() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[roblox]")?;

    for old_solver in [false, true] {
        valid(&check(
            root,
            "--!strict\nreturn function(value: Instance) if value:IsA('Part') then local shape: Enum.PartType = value.Shape print(shape) end local child: Folder? = value:FindFirstChildWhichIsA('Folder') return child end",
            old_solver,
        )?);

        for source in [
            "return Instance.new('Missing')",
            "return Instance.new('Workspace')",
            "return game:GetService('Folder')",
            "return function(value: Instance) return value:IsA('Missing') end",
        ] {
            assert!(check(root, source, old_solver)?.has_errors());
        }
    }

    Ok(())
}

#[test]
fn projects_apply_nested_settings_metadata_and_ignore_patterns() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir_all(root.join("library/sources"))?;

    fs::write(
        root.join("instar.toml"),
        "[roblox]\nproject = 'project.json'",
    )?;

    fs::write(
        root.join("project.json"),
        r#"{"globIgnorePaths":["**/hidden.luau","**/excluded.luau"],"tree":{"$className":"DataModel","Main":{"$path":"main.luau"},"Library":{"$path":"library"}}}"#,
    )?;

    fs::write(
        root.join("library/default.project.json"),
        r#"{"emitLegacyScripts":false,"globIgnorePaths":["!**/hidden.luau"],"tree":{"$path":"sources"}}"#,
    )?;

    fs::write(
        root.join("library/sources/init.meta.json"),
        r#"{"className":"Model"}"#,
    )?;

    fs::write(root.join("library/sources/hidden.luau"), "return 1")?;
    fs::write(root.join("library/sources/excluded.luau"), "return 2")?;

    fs::write(
        root.join("library/sources/worker.client.luau"),
        "print('sample')",
    )?;

    for old_solver in [false, true] {
        valid(&check(
            root,
            "--!strict\nlocal model: Model = game.Library\nlocal worker: Script = game.Library.worker\nlocal value: number = require(game.Library.hidden)\nreturn model, worker, value",
            old_solver,
        )?);

        assert!(
            check(
                root,
                "--!strict\nreturn require(game.Library.excluded)",
                old_solver
            )?
            .has_errors()
        );
    }

    Ok(())
}

#[test]
fn additional_definitions_and_upstream_configuration_remain_separate() -> Result {
    let mut check = checker();
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "definitions = ['types.d.luau']\n[roblox]",
    )?;

    fs::write(
        root.join("types.d.luau"),
        "declare application: {position: Vector3}",
    )?;

    let configuration = root.join("config.luau");

    fs::write(
        &configuration,
        "local configuration: Config = {luau = {languagemode = 'strict'}}\nreturn configuration",
    )?;

    for old_solver in [false, true] {
        valid(&check(
            root,
            "local position: Vector3 = application.position\nreturn position",
            old_solver,
        )?);

        let report = analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            std::slice::from_ref(&configuration),
            &Options {
                old_solver,
                ..Default::default()
            },
        )?;

        valid(&report);
        assert!(!report.documentation.contains_key(&configuration));
    }

    Ok(())
}
