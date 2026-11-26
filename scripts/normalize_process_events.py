"""Convert a Windows process capture into partial shell acceptance evidence."""

import argparse
import json
import ntpath
from pathlib import Path, PureWindowsPath

from shell_acceptance import CONSOLE_CLASSES, PROCESS_ID, SCENARIOS, SHA, SHA256


def filetime(value):
    if not isinstance(value, str) or not value.isascii() or not value.isdecimal():
        raise ValueError(f'Invalid FILETIME {value!r}')
    number = int(value)
    if not 0 < number < 2**64:
        raise ValueError('FILETIME outside its valid range')
    return number


def process_id(value, allow_zero=False):
    if type(value) is not int or not (0 if allow_zero else 1) <= value < 2**32:
        raise ValueError(f'Invalid process identifier {value!r}')
    return value


def image_path(value):
    if not isinstance(value, str) or not PureWindowsPath(value).is_absolute() or '\0' in value:
        raise ValueError('Expected an absolute Windows executable path')
    return ntpath.normcase(ntpath.normpath(value))


def process_identity(record):
    return (process_id(record['pid']), filetime(record['created_filetime']),
            image_path(record['image']))


def verify_process_exit(receipt):
    if (receipt['format'] != 'prismestra-windows-process-exit-v1'
            or receipt['status'] != 'confirmed'):
        raise ValueError('Expected a confirmed process handle exit receipt')
    expected, observed = receipt['expected'], receipt['observed']
    if process_identity(expected) != process_identity(observed):
        raise ValueError('Process handle does not match the expected identity')
    if not (filetime(observed['created_filetime']) <= filetime(receipt['armed_filetime'])
            <= filetime(receipt['exit_filetime']) <= filetime(receipt['confirmed_filetime'])):
        raise ValueError('Process exit receipt has inconsistent times')
    return receipt


def verify_control(control, event_index, harness_pid):
    pid = process_id(control['pid'])
    start = filetime(control['start_filetime'])
    stop = filetime(control['stop_filetime'])
    first = event_index.get(('start', pid, start))
    last = event_index.get(('stop', pid, stop))
    if (stop < start or first is None or last is None
            or first['parent_pid'] != harness_pid
            or any(PureWindowsPath(event['image']).name.casefold() != 'cmd.exe' for event in (first, last))):
        raise ValueError('Capture control lacks its matching command start and stop')
    return start, stop


def normalize(capture, scenario, revision, artifact, product_roots, harness_roots=(),
              process_exit_receipt=None, expected_exit=None):
    if capture['format'] != 'prismestra-windows-process-events-v1':
        raise ValueError('Unsupported Windows process event capture')
    if scenario not in SCENARIOS or not SHA.fullmatch(revision) or not SHA256.fullmatch(artifact):
        raise ValueError('Expected a known scenario, a full Git revision and a lowercase artifact SHA-256')
    started = filetime(capture['started_filetime'])
    ended = filetime(capture['ended_filetime'])
    harness_pid = process_id(capture['harness_pid'])
    event_index = {}
    for event in capture['events']:
        kind = event['kind']
        pid = process_id(event['pid'])
        process_id(event['parent_pid'], allow_zero=True)
        process_id(event['session_id'], allow_zero=True)
        time = filetime(event['filetime'])
        created = event.get('created_filetime')
        if created is not None and (kind != 'start' or filetime(created) > time):
            raise ValueError(f'Invalid process creation time for PID {pid}')
        key = (kind, pid, time)
        if kind not in ('start', 'stop') or key in event_index or not PureWindowsPath(event['image']).name:
            raise ValueError(f'Invalid or repeated process event {key!r}')
        event_index[key] = event
    _, before_stop = verify_control(capture['before_control'], event_index, harness_pid)
    after_start, _ = verify_control(capture['after_control'], event_index, harness_pid)
    if not before_stop <= started < ended <= after_start:
        raise ValueError('Capture controls do not bracket the measured interval')

    active = {}
    processes = {}
    observed = set()
    gaps = []
    events = sorted((time, kind, pid, event) for (kind, pid, time), event in event_index.items()
                    if started <= time < ended)
    for time, kind, pid, event in events:
        if kind == 'stop':
            identity = active.pop(pid, None)
            if identity is not None:
                process = processes[identity]
                process['stopped_filetime'] = str(time)
                if process['image'].casefold() != event['image'].casefold():
                    gaps.append(f'Stop does not match the recorded image for {identity}')
            continue
        if pid in active:
            gaps.append(f'PID {pid} was reused without a captured stop')
            processes[active[pid]]['reused_without_stop'] = True
        parent_pid = event['parent_pid']
        parent = active.get(parent_pid)
        if parent is None and parent_pid in observed:
            gaps.append(f'PID {pid} references an inactive parent {parent_pid}')
        if parent_pid and (('start', parent_pid, time) in event_index
                           or ('stop', parent_pid, time) in event_index):
            gaps.append(f'PID {pid} has an ambiguous parent transition at {time}')
            parent = None
        if parent_pid == pid or ('stop', pid, time) in event_index:
            gaps.append(f'Cannot resolve process lifetime at {pid}:{time}')
            parent = None if parent_pid == pid else parent
        identity = f'{pid}:{time}'
        processes[identity] = {'id': identity, 'parent': parent, 'image': event['image'],
                               'session_id': event['session_id'],
                               'created_filetime': event.get('created_filetime'), 'stopped_filetime': None}
        active[pid] = identity
        observed.add(pid)
    product_roots = list(product_roots)
    harness_roots = list(harness_roots)
    if not product_roots or any(not PROCESS_ID.fullmatch(root) or root not in processes
                                for root in (*product_roots, *harness_roots)):
        raise ValueError('Each declared root must identify a process started in the measured interval')
    if set(product_roots) & set(harness_roots):
        raise ValueError('Product and harness root sets overlap')
    result = {
        'scenario': scenario, 'revision': revision, 'artifact_sha256': artifact,
        'product_roots': product_roots, 'harness_roots': harness_roots,
        'controls': {'process_start_stop': not gaps, 'visible_console': False, 'full_scenario': False},
        'processes': list(processes.values()), 'windows': [], 'process_capture_gaps': gaps,
        'process_interval': {'started_filetime': str(started), 'ended_filetime': str(ended)},
    }
    if (process_exit_receipt is None) != (expected_exit is None):
        raise ValueError('Process exit receipt requires a separate expected launch identity')
    if process_exit_receipt is not None:
        receipt = verify_process_exit(process_exit_receipt)
        if process_identity(receipt['observed']) != process_identity(expected_exit):
            raise ValueError('Process exit receipt does not match the current launch identity')
        if not started <= filetime(receipt['armed_filetime']) <= filetime(receipt['exit_filetime']) < ended:
            raise ValueError('Identified process exit is outside the capture interval')
        # A handle proves only this process exited. It cannot replace a trace event.
        result['identified_process_exit'] = receipt
    return result


def verify_window_controls(capture, witnesses, started, ended):
    if witnesses['desktop'] != capture['desktop']:
        raise ValueError('Visible controls name a different desktop or session')
    controls = {}
    identities = set()
    for phase in ('before', 'after'):
        witness = witnesses[phase]
        pid = process_id(witness['window_owner_pid'])
        created = filetime(witness['window_owner_created_filetime'])
        time = filetime(witness['observed_filetime'])
        identity = (witness['window'], pid, created)
        if identity in identities:
            raise ValueError('Visible controls require two distinct window identities')
        identities.add(identity)
        if not (created <= time and (time < started if phase == 'before' else ended <= time)):
            raise ValueError(f'Visible {phase} control is outside its expected phase')
        matches = [
            row for row in capture['observations']
            if row['reason'] == 'show' and row['class'].casefold() in CONSOLE_CLASSES
            and all(row[key] == witness[key] for key in (
                'window', 'window_owner_pid', 'window_owner_created_filetime', 'observed_filetime'))
        ]
        if len(matches) != 1:
            raise ValueError(f'Visible {phase} control lacks one matching console show event')
        controls[phase] = matches[0]
    return {'desktop': capture['desktop'], **controls}


def add_window_capture(trace, capture, witnesses=None):
    if capture['format'] != 'prismestra-windows-console-windows-v1':
        raise ValueError('Unsupported Windows console window capture')
    started = filetime(trace['process_interval']['started_filetime'])
    ended = filetime(trace['process_interval']['ended_filetime'])
    window_start = filetime(capture['started_filetime'])
    window_end = filetime(capture['ended_filetime'])
    session = process_id(capture['desktop']['session_id'])
    if not window_start <= started < ended <= window_end:
        raise ValueError('The window capture does not enclose the measured process interval')

    owners = {}
    for process in trace['processes']:
        created = process['created_filetime']
        if (created is not None and process['session_id'] == session
                and not process.get('reused_without_stop')):
            pid = int(process['id'].split(':')[0])
            owners.setdefault((pid, filetime(created)), []).append(process)

    windows = list(trace['windows'])
    outside_interval = {'before': [], 'after': []}
    gaps = [*trace.get('window_capture_gaps', ()), *capture['window_capture_gaps']]
    for observation in capture['observations']:
        time = filetime(observation['observed_filetime'])
        if not window_start <= time <= window_end:
            raise ValueError('Window observation lies outside its capture')
        name = observation['class']
        if name.casefold() not in CONSOLE_CLASSES or observation['reason'] not in ('initial', 'final', 'show'):
            raise ValueError('Invalid visible console observation')
        pid = observation['window_owner_pid']
        created = observation['window_owner_created_filetime']
        if pid is not None:
            process_id(pid)
        creation_time = filetime(created) if created is not None else None
        if creation_time is not None and (pid is None or creation_time > time):
            raise ValueError('Invalid window owner creation time')
        if not started <= time < ended:
            phase = 'before' if time < started else 'after'
            outside_interval[phase].append(observation)
            continue
        matches = owners.get((pid, creation_time), ())
        owner = None
        if len(matches) == 1:
            process = matches[0]
            stop = process['stopped_filetime']
            if stop is None or time < filetime(stop):
                owner = process['id']
        if owner is None:
            gaps.append(f'Could not attribute visible window {observation["window"]} to a captured process instance')
        windows.append({'process': owner, 'class': name, 'visible': True})
    controls = {**trace['controls'], 'visible_console': False, 'full_scenario': False}
    context = {'desktop': capture['desktop'], **outside_interval}
    result = {
        **trace, 'controls': controls, 'windows': windows, 'window_capture_gaps': gaps,
        'window_boundary_context': [*trace.get('window_boundary_context', ()), context],
    }
    if witnesses is not None:
        verified = verify_window_controls(capture, witnesses, started, ended)
        result['visible_control_observations'] = [*trace.get('visible_control_observations', ()), verified]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path)
    parser.add_argument('--scenario', required=True, choices=sorted(SCENARIOS))
    parser.add_argument('--revision', required=True)
    parser.add_argument('--artifact-sha256', required=True)
    parser.add_argument('--product-root', required=True, action='append')
    parser.add_argument('--harness-root', default=[], action='append')
    parser.add_argument('--process-exit-receipt', type=Path)
    parser.add_argument('--expect-exit-pid', type=int)
    parser.add_argument('--expect-exit-created-filetime')
    parser.add_argument('--expect-exit-image')
    parser.add_argument('--window-capture', type=Path, action='append', default=[])
    parser.add_argument('--controlled-window-capture', nargs=2, type=Path, action='append', default=[],
                        metavar=('CAPTURE', 'WITNESSES'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    expected_exit = {'pid': args.expect_exit_pid, 'created_filetime': args.expect_exit_created_filetime,
                     'image': args.expect_exit_image}
    if args.process_exit_receipt is None:
        if any(value is not None for value in expected_exit.values()):
            parser.error('Expected launch identity fields require --process-exit-receipt')
        expected_exit = None
    elif any(value is None for value in expected_exit.values()):
        parser.error('Supply all three expected launch identity fields with --process-exit-receipt')
    try:
        capture = json.loads(args.capture.read_text(encoding='utf-8-sig'))
        process_exit = (None if args.process_exit_receipt is None else
                        json.loads(args.process_exit_receipt.read_text(encoding='utf-8-sig')))
        result = normalize(capture, args.scenario, args.revision, args.artifact_sha256,
                           args.product_root, args.harness_root, process_exit, expected_exit)
        for path in args.window_capture:
            windows = json.loads(path.read_text(encoding='utf-8-sig'))
            result = add_window_capture(result, windows)
        for capture_path, witnesses_path in args.controlled_window_capture:
            windows = json.loads(capture_path.read_text(encoding='utf-8-sig'))
            witnesses = json.loads(witnesses_path.read_text(encoding='utf-8-sig'))
            result = add_window_capture(result, windows, witnesses)
        if args.output is not None:
            with args.output.open('x', encoding='utf-8') as output:
                json.dump(result, output, indent=2)
                output.write('\n')
    except (OSError, KeyError, TypeError, ValueError) as error:
        parser.exit(2, f'Invalid process capture: {error}\n')
    if args.output is None:
        print(json.dumps(result, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
