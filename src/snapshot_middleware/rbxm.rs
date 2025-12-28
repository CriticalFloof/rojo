use std::path::Path;

use anyhow::Context;
use memofs::Vfs;

use crate::{
    snapshot::{InstanceContext, InstanceMetadata, InstanceSnapshot},
    syncback::{FsSnapshot, SyncbackReturn, SyncbackSnapshot},
};

use super::{
    dir::{snapshot_dir_no_meta, syncback_dir_no_meta},
    meta_file::{DirectoryMetadata}
};

#[profiling::function]
pub fn snapshot_rbxm(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let temp_tree = rbx_binary::from_reader(vfs.read(path)?.as_slice())
        .with_context(|| format!("Malformed rbxm file: {}", path.display()))?;

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

pub fn snapshot_rbxm_init(
    context: &InstanceContext,
    vfs: &Vfs,
    init_path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let folder_path = init_path.parent().unwrap();
    let dir_snapshot = snapshot_dir_no_meta(context, vfs, folder_path, name)?.unwrap();

    if dir_snapshot.class_name != "Folder" {
        anyhow::bail!(
            "init.rbxm can only be used if the instance produced by \
             the containing directory would be a Folder.\n\
             \n\
             The directory {} turned into an instance of class {}.",
            folder_path.display(),
            dir_snapshot.class_name
        );
    }

    let mut init_snapshot =
        snapshot_rbxm(context, vfs, init_path, &dir_snapshot.name)?.unwrap();

    init_snapshot.children = dir_snapshot.children;
    init_snapshot.metadata = dir_snapshot.metadata;
    // The directory snapshot middleware includes all possible init paths
    // so we don't need to add it here.

    DirectoryMetadata::read_and_apply_all(vfs, folder_path, &mut init_snapshot)?;

    Ok(Some(init_snapshot))
}

pub fn syncback_rbxm<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
) -> anyhow::Result<SyncbackReturn<'sync>> {
    let new_inst = snapshot.new_inst();

    // Long-term, we probably want to have some logic for if this contains a
    // script. That's a future endeavor though.
    let mut serialized = Vec::new();
    rbx_binary::to_writer(&mut serialized, snapshot.new_tree(), &[new_inst.referent()])
        .context("failed to serialize new rbxm")?;

    Ok(SyncbackReturn {
        fs_snapshot: FsSnapshot::new().with_added_file(&snapshot.path, serialized),
        children: Vec::new(),
        removed_children: Vec::new(),
    })
}

pub fn syncback_rbxm_init<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
) -> anyhow::Result<SyncbackReturn<'sync>> {
    let new_inst = snapshot.new_inst();

    let mut serialized = Vec::new();
    rbx_binary::to_writer(&mut serialized, snapshot.new_tree(), &[new_inst.referent()])
        .context("failed to serialize new rbxm")?;

    let mut dir_syncback = syncback_dir_no_meta(snapshot)?;
    dir_syncback.fs_snapshot.add_file(
        snapshot.path.join("init.rbxm"),
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

    #[test]
    fn model_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.rbxm",
            VfsSnapshot::file(include_bytes!("../../assets/test-folder.rbxm").to_vec()),
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_rbxm(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.rbxm"),
            "foo",
        )
        .unwrap()
        .unwrap();

        assert_eq!(instance_snapshot.name, "foo");
        assert_eq!(instance_snapshot.class_name, "Folder");
        assert_eq!(instance_snapshot.children, Vec::new());

        // We intentionally don't assert on properties. rbx_binary does not
        // distinguish between String and BinaryString. The sample model was
        // created by Roblox Studio and has an empty BinaryString "Tags"
        // property that currently deserializes incorrectly.
        // See: https://github.com/Roblox/rbx-dom/issues/49
    }
}
