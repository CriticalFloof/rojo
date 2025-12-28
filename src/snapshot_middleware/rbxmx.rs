use std::path::Path;

use anyhow::Context;
use memofs::Vfs;
use rbx_xml::EncodeOptions;

use crate::{
    snapshot::{InstanceContext, InstanceMetadata, InstanceSnapshot},
    syncback::{FsSnapshot, SyncbackReturn, SyncbackSnapshot},
};

use super::{
    dir::{snapshot_dir_no_meta, syncback_dir_no_meta},
    meta_file::{DirectoryMetadata}
};

pub fn snapshot_rbxmx(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let options = rbx_xml::DecodeOptions::new()
        .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown);

    let temp_tree = rbx_xml::from_reader(vfs.read(path)?.as_slice(), options)
        .with_context(|| format!("Malformed rbxmx file: {}", path.display()))?;

    let root_instance = temp_tree.root();
    let children = root_instance.children();

    if children.len() == 1 {
        let child = children[0];
        let snapshot = InstanceSnapshot::from_tree(temp_tree, child)
            .name(name)
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(vec![path.to_path_buf()])
                    .context(context),
            );

        Ok(Some(snapshot))
    } else {
        anyhow::bail!(
            "Rojo currently only supports model files with one top-level instance.\n\n \
             Check the model file at path {}",
            path.display()
        );
    }
}

pub fn snapshot_rbxmx_init(
    context: &InstanceContext,
    vfs: &Vfs,
    init_path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let folder_path = init_path.parent().unwrap();
    let dir_snapshot = snapshot_dir_no_meta(context, vfs, folder_path, name)?.unwrap();

    if dir_snapshot.class_name != "Folder" {
        anyhow::bail!(
            "init.rbxmx can only be used if the instance produced by \
             the containing directory would be a Folder.\n\
             \n\
             The directory {} turned into an instance of class {}.",
            folder_path.display(),
            dir_snapshot.class_name
        );
    }

    let mut init_snapshot =
        snapshot_rbxmx(context, vfs, init_path, &dir_snapshot.name)?.unwrap();

    println!("INIT RESULT!!1 {}", init_snapshot.snapshot_id);

    init_snapshot.children = dir_snapshot.children;
    init_snapshot.metadata = dir_snapshot.metadata;
    // The directory snapshot middleware includes all possible init paths
    // so we don't need to add it here.

    println!("INIT RESULT!!2 {}", init_snapshot.snapshot_id);

    DirectoryMetadata::read_and_apply_all(vfs, folder_path, &mut init_snapshot)?;

    println!("INIT RESULT!!3 {}", init_snapshot.snapshot_id);

    Ok(Some(init_snapshot))
}

pub fn syncback_rbxmx<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
) -> anyhow::Result<SyncbackReturn<'sync>> {
    let inst = snapshot.new_inst();

    let options =
        EncodeOptions::new().property_behavior(rbx_xml::EncodePropertyBehavior::WriteUnknown);

    // Long-term, we probably want to have some logic for if this contains a
    // script. That's a future endeavor though.
    let mut serialized = Vec::new();
    rbx_xml::to_writer(
        &mut serialized,
        snapshot.new_tree(),
        &[inst.referent()],
        options,
    )
    .context("failed to serialize new rbxmx")?;

    Ok(SyncbackReturn {
        fs_snapshot: FsSnapshot::new().with_added_file(&snapshot.path, serialized),
        children: Vec::new(),
        removed_children: Vec::new(),
    })
}

pub fn syncback_rbxmx_init<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
) -> anyhow::Result<SyncbackReturn<'sync>> {
    let new_inst = snapshot.new_inst();

    let mut serialized = Vec::new();
    rbx_binary::to_writer(&mut serialized, snapshot.new_tree(), &[new_inst.referent()])
        .context("failed to serialize new rbxm")?;

    let mut dir_syncback = syncback_dir_no_meta(snapshot)?;
    dir_syncback.fs_snapshot.add_file(
        snapshot.path.join("init.rbxmx"),
        serialized,
    );
    /* 
    let meta = DirectoryMetadata::from_syncback_snapshot(snapshot, snapshot.path.clone())?;
    if let Some(mut meta) = meta {
        // LocalizationTables have relatively few properties that we care
        // about, so shifting is fine.
        meta.properties.shift_remove(&ustr("Contents"));
        if !meta.is_empty() {
            dir_syncback.fs_snapshot.add_file(
                snapshot.path.join("init.meta.json"),
                serde_json::to_vec_pretty(&meta)
                    .context("could not serialize new init.meta.json")?,
            );
        }
    }
    */

    Ok(dir_syncback)
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};
    use rbx_dom_weak::types::Ref;

    #[test]
    fn plain_folder() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.rbxmx",
            VfsSnapshot::file(
                r#"
                    <roblox version="4">
                        <Item class="Folder" referent="0">
                            <Properties>
                                <string name="Name">THIS NAME IS IGNORED</string>
                            </Properties>
                        </Item>
                    </roblox>
                "#,
            ),
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_rbxmx(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.rbxmx"),
            "foo",
        )
        .unwrap()
        .unwrap();

        assert_eq!(instance_snapshot.name, "foo");
        assert_eq!(instance_snapshot.class_name, "Folder");
        assert_eq!(instance_snapshot.properties, Default::default());
        assert_eq!(instance_snapshot.children, Vec::new());
    }

    #[test]
    fn xml_model_init() {
        let mut imfs = InMemoryFs::new();

        imfs.load_snapshot(
            "/root",
            VfsSnapshot::dir([(
                "init.rbxmx",
                VfsSnapshot::file(
                r#"
                    <roblox version="4">
                        <Item class="Model" referent="0">
                            <Properties>
                                <string name="Source">THIS IS TEST</string>
                            </Properties>
                        </Item>
                    </roblox>
                "#,
                ),
            )])
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_rbxmx_init(
            &InstanceContext::default(),
            &vfs,
            Path::new("/root/init.rbxmx"),
            "root",
        ).unwrap().unwrap().snapshot_id(Ref::none());

        println!("FINAL RESULT!! {}", instance_snapshot.class_name);

        insta::with_settings!({ sort_maps => true }, {
            insta::assert_yaml_snapshot!(instance_snapshot);
        });
    }
}
