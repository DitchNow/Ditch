import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/main.dart';

Map<String, dynamic> _agent(String id, String projectId) => {
  'id': id,
  'project_id': projectId,
  'state': 'Completed',
  'started_at': '2026-01-01T00:00:00Z',
  'updated_at': '2026-01-01T00:00:00Z',
};

class _SnapshotClient extends DitchRuntimeClient {
  _SnapshotClient() : super(socketPath: '/test/runtime.sock');
  final events = StreamController<Map<String, dynamic>>();
  final agents = [
    _agent('agent-a', 'project-a'),
    _agent('agent-b', 'project-b'),
  ];

  Map<String, dynamic> get state => {
    'projects': [
      {'id': 'project-a', 'name': 'Project A', 'root': '/test/a'},
      {'id': 'project-b', 'name': 'Project B', 'root': '/test/b'},
    ],
    'agents': List<Map<String, dynamic>>.of(agents),
    'tasks': [],
    'attention': [],
    'permissions': [],
  };

  void emit(Map<String, dynamic> event) => events.add({'event': event});
  void refresh() => emit({'SnapshotReplaced': state});

  @override
  Future<Map<String, dynamic>> snapshot() async => {'Snapshot': state};

  @override
  Future<Stream<Map<String, dynamic>>> subscribeEvents() async => events.stream;

  @override
  Future<RuntimeStatusDto> runtimeStatus() async => const RuntimeStatusDto(
    identity: 'Test Runtime',
    deploymentEnvironment: 'production',
    pid: 1,
    socketPath: '/test/runtime.sock',
    activeSessionCount: 0,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'test-runtime',
    capabilities: {
      'app_server_sessions_v2',
      'persistent_sessions_v1',
      'always_on_web_access_v1',
    },
  );

  @override
  Future<Map<String, dynamic>> request(Object body) async {
    if (body is Map && body.containsKey('ListAgentMessages')) {
      return {
        'AgentMessages': {'messages': [], 'has_more': false},
      };
    }
    throw const DitchRuntimeException(
      'test_unavailable',
      'Unavailable in test',
    );
  }
}

class _HistoryClient extends _SnapshotClient {
  final history = Completer<Map<String, dynamic>>();
  final cursors = <int?>[];

  Map<String, dynamic> row(int sequence, String role) => {
    'sequence': sequence,
    'message': {
      'agent_id': 'agent-a',
      'role': role,
      'text': 'Saved message $sequence',
      'created_at': DateTime.utc(
        2026,
      ).add(Duration(seconds: sequence)).toIso8601String(),
    },
  };

  @override
  Future<Map<String, dynamic>> request(Object body) async {
    if (body is Map && body['ListAgentMessages'] is Map) {
      final cursor = body['ListAgentMessages']['before_sequence'] as int?;
      cursors.add(cursor);
      if (cursor != null) return history.future;
      return {
        'AgentMessages': {
          'messages': [
            row(101, 'Tool'),
            for (var i = 102; i <= 130; i++) row(i, 'Assistant'),
          ],
          'has_more': true,
          'next_before_sequence': 101,
        },
      };
    }
    return super.request(body);
  }
}

void main() {
  testWidgets(
    'runtime history and a concurrent live event retain their provenance',
    (tester) async {
      tester.view.physicalSize = const Size(1440, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        (call) async => switch (call.method) {
          'runtimeAvailable' => true,
          'getPaneWidths' => <String, double>{},
          _ => null,
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      final client = _HistoryClient();
      addTearDown(client.events.close);
      await tester.pumpWidget(
        MaterialApp(home: CommandCenterScreen(runtimeClient: client)),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('agent-header-agent-a')));
      await tester.pumpAndSettle();
      ConversationTranscript transcript() => tester
          .widget<ConversationTranscript>(find.byType(ConversationTranscript));
      final viewport = transcript().viewport;
      expect(client.cursors, [null]);
      expect(
        transcript().messages.every(
          (m) => m.arrival == ChatMessageArrival.history,
        ),
        isTrue,
      );
      await tester.drag(
        find.descendant(
          of: find.byType(ConversationTranscript),
          matching: find.byType(ListView),
        ),
        const Offset(0, 250),
      );
      await tester.pumpAndSettle();
      for (var i = 0; i < 3; i++) {
        viewport.scrollController.jumpTo(
          viewport.scrollController.position.maxScrollExtent - 30,
        );
        await tester.pumpAndSettle();
      }
      expect(viewport.isNearOldest, isTrue);
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: tester.getCenter(
            find.descendant(
              of: find.byType(ConversationTranscript),
              matching: find.byType(ListView),
            ),
          ),
          scrollDelta: const Offset(0, -20),
        ),
      );
      await tester.pump();
      expect(client.cursors, [null, 101]);
      client.emit({
        'AgentMessageAppended': {
          'id': 'live-result',
          'agent_id': 'agent-a',
          'role': 'Assistant',
          'text': 'Live result while loading history',
          'created_at': '2026-02-01T00:00:00Z',
        },
      });
      client.history.complete({
        'AgentMessages': {
          'messages': [
            client.row(1, 'User'),
            client.row(2, 'Tool'),
            for (var i = 3; i <= 100; i++) client.row(i, 'Assistant'),
          ],
          'has_more': false,
        },
      });
      await tester.pumpAndSettle();
      expect(viewport.unseenCount, 1);
      expect(client.cursors, [null, 101]);
      expect(
        transcript().messages
            .singleWhere((m) => m.identity == 'live-result')
            .arrival,
        ChatMessageArrival.live,
      );
      expect(
        transcript().messages
            .where((m) => m.identity != 'live-result')
            .every((m) => m.arrival == ChatMessageArrival.history),
        isTrue,
      );
      client.refresh();
      await tester.pumpAndSettle();
      expect(viewport.unseenCount, 1);
      expect(client.cursors, [null, 101]);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
    },
  );

  testWidgets('background updates preserve explicit agent expansion choices', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1440, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    const channel = MethodChannel('the_ditch/application');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async => switch (call.method) {
        'runtimeAvailable' => true,
        'getPaneWidths' => <String, double>{},
        _ => null,
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    final client = _SnapshotClient();
    addTearDown(client.events.close);
    await tester.pumpWidget(
      MaterialApp(home: CommandCenterScreen(runtimeClient: client)),
    );
    await tester.pumpAndSettle();

    void expectCollapsed(String id) {
      expect(find.byKey(ValueKey('agent-header-$id')), findsOneWidget);
      expect(find.byType(AgentChatPanel), findsNothing);
    }

    // Initial hydration and repeated background snapshots keep the list closed.
    expectCollapsed('agent-a');
    for (var i = 0; i < 2; i++) {
      client.refresh();
      await tester.pumpAndSettle(const Duration(seconds: 3));
      expectCollapsed('agent-a');
    }

    // An explicit open survives refresh; an explicit collapse does too.
    await tester.tap(find.byKey(const ValueKey('agent-header-agent-a')));
    await tester.pumpAndSettle();
    expect(find.byType(AgentChatPanel), findsOneWidget);
    client.refresh();
    await tester.pumpAndSettle();
    expect(find.byType(AgentChatPanel), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('agent-header-agent-a')));
    await tester.pumpAndSettle();
    for (var i = 0; i < 2; i++) {
      client.refresh();
      await tester.pumpAndSettle(const Duration(seconds: 3));
      expectCollapsed('agent-a');
    }

    // Discovering a new agent must not open it.
    final discovered = _agent('agent-new', 'project-a');
    client.agents.add(discovered);
    client.emit({'AgentChanged': discovered});
    await tester.pumpAndSettle();
    expectCollapsed('agent-new');

    // Removing the open agent must not choose another agent for the user.
    await tester.tap(find.byKey(const ValueKey('agent-header-agent-new')));
    await tester.pumpAndSettle();
    expect(find.byType(AgentChatPanel), findsOneWidget);
    client.agents.remove(discovered);
    client.refresh();
    await tester.pumpAndSettle();
    expectCollapsed('agent-a');

    // Switching projects remains collapsed after later runtime snapshots.
    await tester.tap(find.text('Project B'));
    await tester.pumpAndSettle();
    expectCollapsed('agent-b');
    client.refresh();
    await tester.pumpAndSettle(const Duration(seconds: 3));
    expectCollapsed('agent-b');
    expect(tester.takeException(), isNull);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
  });
}
