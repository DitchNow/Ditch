import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class EndpointClient extends DitchRuntimeClient {
  final calls = <Object>[];
  @override
  Future<Map<String, dynamic>> request(Object body) async {
    calls.add(body);
    final operation = ((body as Map)['Ollama'] as Map)['operation'];
    if (operation == 'List') {
      return {
        'Ollama': {
          'Endpoints': [
            {
              'id': 'server-a',
              'display_name': 'GPU A',
              'enabled': true,
              'models': ['qwen:9b'],
            },
            {
              'id': 'server-b',
              'display_name': 'GPU B',
              'enabled': true,
              'models': ['qwen:9b'],
              'error': 'Server unavailable',
            },
          ],
        },
      };
    }
    if ((operation as Map).containsKey('Test')) {
      return {
        'Ollama': {
          'Probe': {
            'models': ['qwen:9b'],
            'reachable': true,
            'responses_tool_check': false,
          },
        },
      };
    }
    return {
      'Ollama': {
        'Endpoint': {
          'id': 'server-c',
          'display_name': 'GPU C',
          'enabled': true,
          'models': ['qwen:9b'],
        },
      },
    };
  }
}

void main() {
  test(
    'old local runtime never receives an OSS launch or endpoint request',
    () async {
      final directory = await Directory.systemTemp.createTemp('oss-');
      final socketPath = '${directory.path}/s';
      final server = await ServerSocket.bind(
        InternetAddress(socketPath, type: InternetAddressType.unix),
        0,
      );
      final received = <Object?>[];
      server.listen((socket) {
        socket
            .cast<List<int>>()
            .transform(utf8.decoder)
            .transform(const LineSplitter())
            .listen((line) {
              received.add((jsonDecode(line) as Map)['body']);
              socket.writeln(
                jsonEncode({
                  'body': {
                    'RuntimeStatus': {
                      'capabilities': [
                        'app_server_sessions_v2',
                        'ollama_inference_v1',
                      ],
                    },
                  },
                }),
              );
              socket.flush().then((_) => socket.destroy());
            });
      });
      addTearDown(() async {
        await server.close();
        await directory.delete(recursive: true);
      });
      final client = DitchRuntimeClient(socketPath: socketPath);
      await expectLater(
        client.startCodexSession(
          projectId: 'project',
          projectName: 'Project',
          projectRoot: '/project',
          prompt: 'test',
          executionProfile: {
            'model': 'qwen:9b',
            'inference_endpoint_id': 'server-a',
          },
        ),
        throwsFormatException,
      );
      await expectLater(
        client.ollamaRequest('project', 'List'),
        throwsFormatException,
      );
      expect(received, ['RuntimeStatus', 'RuntimeStatus']);
    },
  );
  test(
    'duplicate model IDs have distinct selections and persisted endpoints',
    () async {
      final settings = AgentExecutionSettings();
      await settings.loadOssModels(EndpointClient(), 'project');
      expect(settings.ossModels.map((m) => m.selectionKey).toSet().length, 2);
      settings.selectOption(settings.ossModels.last);
      expect(settings.protocolValue['model'], 'qwen:9b');
      expect(settings.protocolValue['inference_endpoint_id'], 'server-b');
      expect(settings.selectedModel?.endpointName, 'GPU B');
      expect(settings.selectedModel?.availabilityError, 'Server unavailable');
    },
  );
  test(
    'saved sessions reject provider changes without contaminating defaults',
    () {
      final defaults = AgentExecutionSettings();
      const a = AgentModelOption(
        id: 'qwen:9b',
        displayName: 'Qwen',
        endpointId: 'server-a',
      );
      const b = AgentModelOption(
        id: 'qwen:9b',
        displayName: 'Qwen',
        endpointId: 'server-b',
      );
      defaults.selectOption(a);
      final saved = AgentExecutionSettings(profile: defaults.protocolValue);
      defaults.selectOption(b);
      saved.selectOption(b);
      saved.selectOption(null);
      expect(saved.inferenceEndpointId, 'server-a');
      expect(defaults.inferenceEndpointId, 'server-b');
      expect(
        AgentExecutionSettings(profile: {'model': 'gpt-test'}).canSelect(a),
        isFalse,
      );
    },
  );
  test('OSS catalog remains available when OpenAI discovery fails', () async {
    final settings = AgentExecutionSettings();
    await Future.wait([
      settings.loadModels(() async => throw Exception('not signed in')),
      settings.loadOssModels(EndpointClient(), 'project'),
    ]);
    expect(settings.modelError, isNotNull);
    expect(settings.ossModels.length, 2);
    expect(settings.ossError, isNull);
  });
  test('OSS remote readiness is separate from OpenAI readiness', () {
    final status = <String, dynamic>{
      'ready': false,
      'checks': [
        {'key': 'oss_execution', 'state': 'ready'},
      ],
    };
    expect(
      remoteSetupReadyForExecution(status, requireCodexSandbox: false),
      isFalse,
    );
    expect(
      remoteSetupReadyForExecution(
        status,
        requireCodexSandbox: false,
        requireOpenAIAuth: false,
      ),
      isTrue,
    );
    expect(
      remoteSetupReadyForExecution(
        {'ready': false},
        requireCodexSandbox: false,
        requireOpenAIAuth: false,
      ),
      isFalse,
    );
  });
  testWidgets(
    'shared picker exposes groups and cancellation preserves selection',
    (tester) async {
      final settings = AgentExecutionSettings();
      final client = EndpointClient();
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: AgentExecutionControls(
              settings: settings,
              runtimeClient: client,
              projectId: 'project',
              loadModels: () async => [],
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byType(AgentModelPicker));
      await tester.pumpAndSettle();
      expect(find.text('OpenAI models'), findsOneWidget);
      expect(find.text('OSS models'), findsOneWidget);
      await tester.tap(find.text('OSS models'));
      await tester.pumpAndSettle();
      expect(find.text('GPU A'), findsOneWidget);
      expect(find.text('GPU B'), findsOneWidget);
      await tester.tap(find.text('Add OSS model…').last);
      await tester.pumpAndSettle();
      expect(find.byType(AddOssModelDialog), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(settings.model, isNull);
      expect(client.calls.length, 1);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'modal sends credentials only in registration and returns model plus endpoint',
    (tester) async {
      final client = EndpointClient();
      AgentModelOption? selected;
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () async {
                  selected = await showDialog<AgentModelOption>(
                    context: context,
                    builder: (_) =>
                        AddOssModelDialog(client: client, projectId: 'project'),
                  );
                },
                child: const Text('Open'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      await tester.enterText(find.widgetWithText(TextField, 'Name'), 'GPU C');
      await tester.enterText(
        find.widgetWithText(TextField, 'Ollama API URL'),
        'https://gpu.example',
      );
      await tester.enterText(
        find.widgetWithText(TextField, 'API token (optional)'),
        'test-secret',
      );
      await tester.enterText(
        find.widgetWithText(TextField, 'Exact model ID'),
        'qwen:9b',
      );
      await tester.tap(find.text('Add').last);
      await tester.pumpAndSettle();
      expect(selected?.endpointId, 'server-c');
      expect(selected?.id, 'qwen:9b');
      final settings = AgentExecutionSettings()..selectOption(selected);
      expect(settings.protocolValue.toString(), isNot(contains('test-secret')));
      expect(tester.takeException(), isNull);
    },
  );
}
