use std::path::PathBuf;

use roxmltree::{Document, Node, ParsingOptions};

use super::{
    native::{err, resolve_account, split_arguments, task_owner},
    retirement::Launch,
};

const TASK_NAMESPACE: &str = "http://schemas.microsoft.com/windows/2004/02/mit/task";

pub(super) fn describe(xml: &str, security: &str) -> Result<(Launch, String), String> {
    let doc = Document::parse_with_options(
        xml,
        ParsingOptions {
            nodes_limit: 4096,
            ..ParsingOptions::default()
        },
    )
    .map_err(err)?;
    let task = doc.root_element();
    if !task.has_tag_name((TASK_NAMESPACE, "Task")) {
        return Err("unrecognized scheduled task XML".into());
    }
    let actions = required(task, "Actions")?;
    let exec = only(actions, "Exec")?;
    if exec.children().filter(Node::is_element).count() != 3 {
        return Err("unrecognized scheduled Exec fields".into());
    }
    let trigger = only(required(task, "Triggers")?, "LogonTrigger")?;
    let principal = only(required(task, "Principals")?, "Principal")?;
    if actions
        .attribute("Context")
        .is_some_and(|id| principal.attribute("id") != Some(id))
        || value(principal, "LogonType")? != Some("InteractiveToken")
        || child(principal, "GroupId")?.is_some()
        || child(principal, "RequiredPrivileges")?.is_some()
        || !boolean(trigger, "Enabled", true)?
    {
        return Err("unrecognized scheduled task principal or trigger".into());
    }
    let elevated = match value(principal, "RunLevel")? {
        None | Some("LeastPrivilege") => false,
        Some("HighestAvailable") => true,
        _ => return Err("unrecognized scheduled task run level".into()),
    };
    // Persisted XML can omit the default Enabled setting independently of the
    // registered enabled bit. Preserve every other field and its raw spelling.
    let settings = child(task, "Settings")?;
    let comparison = match settings
        .map(|settings| child(settings, "Enabled"))
        .transpose()?
        .flatten()
    {
        Some(enabled) => {
            if enabled.attributes().len() != 0
                || !matches!(text(enabled)?.trim(), "0" | "1" | "false" | "true")
            {
                return Err("unrecognized task settings enabled value".into());
            }
            without_enabled(xml, enabled)
        }
        None => xml.to_owned(),
    };
    let launch = Launch {
        principal_sid: resolve_account(required_value(principal, "UserId")?)?,
        trigger_sid: resolve_account(required_value(trigger, "UserId")?)?,
        owner_sid: task_owner(security)?,
        elevated,
        executable: PathBuf::from(required_value(exec, "Command")?),
        arguments: split_arguments(required_value(exec, "Arguments")?)?,
        directory: PathBuf::from(required_value(exec, "WorkingDirectory")?),
    };
    Ok((launch, comparison))
}

fn child<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &str,
) -> Result<Option<Node<'a, 'input>>, String> {
    let mut matching = node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == tag);
    let result = matching.next();
    if matching.next().is_some()
        || result.is_some_and(|n| n.tag_name().namespace() != Some(TASK_NAMESPACE))
    {
        return Err(format!("ambiguous task XML element {tag}"));
    }
    Ok(result)
}

fn required<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Result<Node<'a, 'input>, String> {
    child(node, tag)?.ok_or_else(|| format!("missing task XML element {tag}"))
}

fn only<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Result<Node<'a, 'input>, String> {
    let candidate = required(node, tag)?;
    if node.children().filter(Node::is_element).count() != 1 {
        return Err(format!("expected a single {tag}"));
    }
    Ok(candidate)
}

fn text<'a>(node: Node<'a, '_>) -> Result<&'a str, String> {
    let mut children = node.children();
    let value = children.next().filter(Node::is_text).and_then(|n| n.text());
    if children.next().is_some() || value.is_none() {
        return Err("expected one scheduled task text value".into());
    }
    Ok(value.unwrap_or_default())
}

fn value<'a>(node: Node<'a, '_>, tag: &str) -> Result<Option<&'a str>, String> {
    child(node, tag)?.map(text).transpose()
}

fn required_value<'a>(node: Node<'a, '_>, tag: &str) -> Result<&'a str, String> {
    value(node, tag)?.ok_or_else(|| format!("missing task XML value {tag}"))
}

fn boolean(node: Node<'_, '_>, tag: &str, default: bool) -> Result<bool, String> {
    match value(node, tag)?.map(str::trim) {
        None => Ok(default),
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        _ => Err(format!("invalid boolean task XML value {tag}")),
    }
}

fn without_enabled(xml: &str, node: Node<'_, '_>) -> String {
    let range = node.range();
    let mut start = range.start;
    let mut end = range.end;
    let indent = xml[..start].rfind('\n').map_or(0, |index| index + 1);
    // If the scheduler inserted the default as its own line, disregard only
    // that line. Other whitespace and nested trigger settings remain evidence.
    if xml[indent..start]
        .bytes()
        .all(|b| matches!(b, b' ' | b'\t'))
        && let Some(newline) = xml[end..].find('\n')
        && xml[end..end + newline]
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
    {
        start = indent;
        end += newline + 1;
    }
    let mut comparison = xml.to_owned();
    comparison.replace_range(start..end, "");
    comparison
}

#[cfg(test)]
mod tests {
    use super::*;
    const SID: &str = "S-1-5-21-1-2-3-1001";
    const ACL: &str = "O:SYD:(A;;FA;;;SY)(A;;FA;;;BA)";

    fn task() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task xmlns="{TASK_NAMESPACE}" version="1.4">
    <Principals><Principal id="Author"><UserId>{SID}</UserId><LogonType>InteractiveToken</LogonType></Principal></Principals>
    <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{SID}</UserId><Delay>PT15S</Delay></LogonTrigger></Triggers>
    <Settings>
        <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    </Settings>
    <Actions Context="Author"><Exec>
        <Command>C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe</Command>
        <Arguments>-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "C:\Legacy source\Start Prismestra.ps1" -Autostart</Arguments>
        <WorkingDirectory>C:\Legacy source</WorkingDirectory>
    </Exec></Actions>
</Task>"#
        )
    }

    #[test]
    fn recognized_xml_is_lossless_except_direct_task_enabled_setting() {
        let xml = task();
        let (launch, normalized) = describe(&xml, ACL).unwrap();
        assert_eq!(launch.principal_sid, SID);
        assert_eq!(launch.trigger_sid, SID);
        let highest = xml.replace("-Autostart", "-BackendOnly").replace(
            "</Principal>",
            "<RunLevel>HighestAvailable</RunLevel></Principal>",
        );
        let (backend, _) = describe(&highest, ACL).unwrap();
        assert!(backend.elevated);
        assert!(backend.launcher(SID, &backend.executable).is_some());
        assert_eq!(normalized, xml);
        for value in ["true", "false"] {
            let changed = xml.replace(
                "<Settings>\n",
                &format!("<Settings>\n        <Enabled>{value}</Enabled>\n"),
            );
            assert_eq!(describe(&changed, ACL).unwrap().1, normalized);
        }
        let changed = xml.replace("PT15S", "PT10S");
        assert_ne!(describe(&changed, ACL).unwrap().1, normalized);
        assert!(describe(&xml.replace("<Enabled>true", "<Enabled>false"), ACL).is_err());
    }

    #[test]
    fn unfamiliar_task_shapes_and_missing_security_owner_are_preserved() {
        let xml = task();
        for changed in [
            xml.replace("</Exec>", "</Exec><Exec/>"),
            xml.replace("</LogonTrigger>", "</LogonTrigger><LogonTrigger/>"),
            xml.replace("LogonTrigger", "BootTrigger"),
            xml.replace("Exec", "ComHandler"),
            xml.replace("InteractiveToken", "S4U"),
            xml.replace("Context=\"Author\"", "Context=\"Unknown\""),
            xml.replace(
                "</Principal>",
                "<GroupId>Administrators</GroupId></Principal>",
            ),
            xml.replace(TASK_NAMESPACE, "http://example.test/task"),
            xml.replace("<Enabled>true", "<Enabled>true</Enabled><Enabled>true"),
        ] {
            assert!(describe(&changed, ACL).is_err(), "{changed}");
        }
        assert!(describe(&xml, "D:(A;;FA;;;BA)").is_err());
    }
}
