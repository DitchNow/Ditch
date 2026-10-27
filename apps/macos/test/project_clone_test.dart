import 'dart:async';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/project_clone_controller.dart';
import 'package:the_ditch/data/project_clone_models.dart';
import 'package:the_ditch/data/runtime_transport.dart';
import 'package:the_ditch/main.dart' show AddProjectKindDialog;
import 'package:the_ditch/widgets/clone_project_dialog.dart';

class FakeCloneClient implements ProjectCloneClient {
  String id = '';
  int starts = 0;
  int cancels = 0;
  int retries = 0;
  String phase = 'cloning';
  double? percentage;
  String? message;
  bool retained = false;
  Object? failure;
  Completer<ProjectCloneStatus>? pendingStart;

  ProjectCloneStatus get status => ProjectCloneStatus(
    operationId: id,
    destination: '/projects/repo',
    phase: phase,
    percentage: percentage,
    progressLabel: percentage == null ? null : 'Receiving objects',
    error: message,
    checkoutReady: retained,
    project: phase == 'completed'
        ? {'id': 'project-id', 'name': 'repo', 'root': '/projects/repo'}
        : null,
  );

  @override
  Future<ProjectCloneStatus> startProjectClone({
    required String operationId,
    required String url,
    required String parent,
    required String folder,
    required String name,
  }) async {
    starts++;
    id = operationId;
    if (failure != null) throw failure!;
    return pendingStart == null ? status : pendingStart!.future;
  }

  @override
  Future<ProjectCloneStatus> projectCloneStatus(String operationId) async {
    if (failure != null) throw failure!;
    return status;
  }

  @override
  Future<ProjectCloneStatus> cancelProjectClone(String operationId) async {
    cancels++;
    if (failure != null) throw failure!;
    return status;
  }

  @override
  Future<ProjectCloneStatus> retryProjectCloneRegistration(
    String operationId,
  ) async {
    retries++;
    phase = 'configuring';
    return status;
  }
}

Future<void> start(ProjectCloneController controller) => controller.start(
  url: 'git@github.com:owner/repo.git',
  parent: '/projects',
  folder: 'repo',
  name: 'repo',
);

void main() {
  test('infers names and rejects credentials, helpers and traversal', () {
    expect(repositoryFolderName('git@github.com:owner/repo.git'), 'repo');
    expect(repositoryFolderName('https://github.com/owner/repo.git/'), 'repo');
    for (final url in [
      'file:///repo',
      '--evil',
      'https://token@github.com/a/b',
      'ssh://u:p@host/r',
      'https://host/repo?token=secret',
    ]) {
      expect(validateCloneFields(url, '/projects', 'repo', 'repo'), isNotNull);
    }
    expect(
      validateCloneFields(
        'git@github.com:owner/repo',
        '/projects',
        '../escape',
        'repo',
      ),
      isNotNull,
    );
    expect(
      validateCloneFields(
        'https://github.com/owner/repo',
        '/projects',
        'repo',
        'repo',
      ),
      isNull,
    );
  });

  test('status requires an actual registered project on completion', () {
    expect(
      () => ProjectCloneStatus.fromResponse({'Accepted': true}),
      throwsFormatException,
    );
    expect(
      () => ProjectCloneStatus.fromResponse({
        'ProjectClone': {
          'operation_id': 'id',
          'destination': '/repo',
          'phase': 'completed',
        },
      }),
      throwsFormatException,
    );
    final parsed = ProjectCloneStatus.fromResponse({
      'ProjectClone': {
        'operation_id': 'id',
        'destination': '/repo',
        'phase': 'cloning',
        'percentage': 42,
        'progress_label': 'Receiving objects',
      },
    });
    expect(parsed.percentage, .42);
    expect(parsed.terminal, isFalse);
  });

  test(
    'duplicate submission is disabled and polling completes exactly once',
    () async {
      final client = FakeCloneClient();
      final controller = ProjectCloneController(
        client,
        pollInterval: const Duration(days: 1),
      );
      addTearDown(controller.dispose);
      var completed = 0;
      controller.addListener(() {
        if (controller.status?.phase == 'completed') completed++;
      });
      await start(controller);
      await start(controller);
      expect(client.starts, 1);
      client.phase = 'completed';
      await controller.poll();
      await controller.poll();
      expect(completed, 1);
      expect(controller.active, isFalse);
      expect(controller.status?.project, isNotNull);
    },
  );

  test(
    'connection loss preserves operation ID and resumes status polling',
    () async {
      final client = FakeCloneClient()..failure = const SocketException('lost');
      final controller = ProjectCloneController(
        client,
        pollInterval: const Duration(days: 1),
      );
      addTearDown(controller.dispose);
      await start(controller);
      final id = controller.operationId;
      expect(controller.active, isTrue);
      expect(controller.connectionLost, isTrue);
      client.failure = null;
      await controller.poll();
      expect(controller.operationId, id);
      expect(client.starts, 1);
      expect(controller.connectionLost, isFalse);
      client.failure = const DitchRuntimeException(
        'clone_unknown',
        'Runtime restarted. Inspect destination.',
      );
      await controller.poll();
      expect(controller.active, isFalse);
      expect(controller.error, contains('Inspect destination'));
    },
  );

  test(
    'cancellation waits for daemon and retained checkout retries registration only',
    () async {
      final client = FakeCloneClient();
      final controller = ProjectCloneController(
        client,
        pollInterval: const Duration(days: 1),
      );
      addTearDown(controller.dispose);
      await start(controller);
      await controller.cancel();
      expect(controller.cancelling, isTrue);
      expect(controller.active, isTrue);
      client.phase = 'cancelled';
      await controller.poll();
      expect(controller.active, isFalse);
      client.phase = 'cloning';
      await start(controller);
      client.phase = 'failed';
      client.retained = true;
      client.message = 'Checkout retained';
      await controller.poll();
      final starts = client.starts;
      await controller.retryRegistration();
      expect(client.starts, starts);
      expect(client.retries, 1);
      expect(controller.status?.phase, 'configuring');
    },
  );

  test('unavailable runtime never becomes a successful addition', () async {
    final client = FakeCloneClient()
      ..failure = const DitchRuntimeException(
        'clone_unavailable',
        'Update the runtime',
      );
    final controller = ProjectCloneController(client);
    addTearDown(controller.dispose);
    await start(controller);
    expect(controller.active, isFalse);
    expect(controller.status?.project, isNull);
    expect(controller.error, contains('Update'));
  });

  test('disposing during a request prevents later notifications', () async {
    final client = FakeCloneClient()
      ..pendingStart = Completer<ProjectCloneStatus>();
    final controller = ProjectCloneController(client);
    final future = start(controller);
    controller.dispose();
    client.pendingStart!.complete(client.status);
    await future;
  });

  testWidgets('chooser includes GitHub alongside Local and Remote', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(home: Scaffold(body: AddProjectKindDialog())),
    );
    expect(find.text('Local Project'), findsOneWidget);
    expect(find.text('Remote Project'), findsOneWidget);
    expect(find.text('GitHub'), findsOneWidget);
  });

  testWidgets(
    'picker cancellation preserves inputs and inferred names remain editable',
    (tester) async {
      final client = FakeCloneClient();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/project_picker'),
        (call) async => null,
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          const MethodChannel('the_ditch/project_picker'),
          null,
        ),
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: CloneProjectDialog(client: client)),
        ),
      );
      final fields = find.byType(TextField);
      await tester.enterText(fields.at(0), 'git@github.com:owner/repo.git');
      await tester.pump();
      expect(tester.widget<TextField>(fields.at(2)).controller!.text, 'repo');
      expect(tester.widget<TextField>(fields.at(3)).controller!.text, 'repo');
      await tester.enterText(fields.at(1), '/projects');
      await tester.tap(find.text('Choose…'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(fields.at(1)).controller!.text,
        '/projects',
      );
      await tester.enterText(fields.at(2), 'custom-folder');
      await tester.enterText(fields.at(3), 'Custom project');
      await tester.enterText(
        fields.at(0),
        'https://github.com/owner/other.git',
      );
      await tester.pump();
      expect(
        tester.widget<TextField>(fields.at(2)).controller!.text,
        'custom-folder',
      );
      expect(
        tester.widget<TextField>(fields.at(3)).controller!.text,
        'Custom project',
      );
      expect(find.text('Destination: /projects/custom-folder'), findsOneWidget);
    },
  );

  testWidgets(
    'modal shows progress, disables controls, preserves failures and returns once',
    (tester) async {
      final client = FakeCloneClient();
      var results = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                child: const Text('Open'),
                onPressed: () async {
                  final result = await showDialog<Map<String, dynamic>>(
                    context: context,
                    barrierDismissible: false,
                    builder: (_) => CloneProjectDialog(client: client),
                  );
                  if (result != null) results++;
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      final fields = find.byType(TextField);
      await tester.enterText(fields.at(0), 'https://github.com/owner/repo.git');
      await tester.enterText(fields.at(1), '/projects');
      await tester.tap(find.text('Clone and add'));
      await tester.pump();
      expect(
        tester
            .widget<LinearProgressIndicator>(
              find.byType(LinearProgressIndicator),
            )
            .value,
        isNull,
      );
      expect(tester.widget<TextField>(fields.first).enabled, isFalse);
      expect(
        tester.widget<FilledButton>(find.byType(FilledButton)).onPressed,
        isNull,
      );
      client.percentage = .65;
      await tester.pump(const Duration(milliseconds: 600));
      await tester.pump();
      expect(
        tester
            .widget<LinearProgressIndicator>(
              find.byType(LinearProgressIndicator),
            )
            .value,
        .65,
      );
      await tester.tap(find.text('Cancel clone'));
      await tester.pump();
      expect(find.text('Cancelling and cleaning up…'), findsOneWidget);
      client.phase = 'failed';
      client.message = 'Authentication failed';
      await tester.pump(const Duration(milliseconds: 600));
      await tester.pump();
      expect(find.text('Authentication failed'), findsOneWidget);
      expect(
        tester.widget<TextField>(fields.first).controller!.text,
        'https://github.com/owner/repo.git',
      );
      client.phase = 'cloning';
      client.message = null;
      await tester.tap(find.text('Clone and add'));
      await tester.pump();
      client.phase = 'completed';
      await tester.pump(const Duration(milliseconds: 600));
      await tester.pumpAndSettle();
      expect(results, 1);
      expect(find.byType(CloneProjectDialog), findsNothing);
    },
  );

  testWidgets(
    'lost connection can be explicitly closed without claiming cancellation',
    (tester) async {
      final client = FakeCloneClient()..failure = const SocketException('lost');
      var results = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                child: const Text('Open'),
                onPressed: () async {
                  final result = await showDialog<Map<String, dynamic>>(
                    context: context,
                    barrierDismissible: false,
                    builder: (_) => CloneProjectDialog(client: client),
                  );
                  if (result != null) results++;
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byType(TextField).at(0),
        'https://github.com/owner/repo.git',
      );
      await tester.enterText(find.byType(TextField).at(1), '/projects');
      await tester.tap(find.text('Clone and add'));
      await tester.pump();
      expect(find.textContaining('may still be running'), findsOneWidget);
      await tester.tap(find.text('Close and check later'));
      await tester.pumpAndSettle();
      expect(find.byType(CloneProjectDialog), findsNothing);
      expect(results, 0);
      expect(client.cancels, 0);
    },
  );
}
