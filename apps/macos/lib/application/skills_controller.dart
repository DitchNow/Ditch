import 'package:flutter/foundation.dart';

typedef SkillsTransport = Future<Map<String, dynamic>> Function(Object request);

class SkillsController extends ChangeNotifier {
  SkillsController({required this.request});
  final SkillsTransport request;
  bool busy = false;
  bool disposed = false;
  String? error;
  int generation = 0;
  int invalidation = 0;
  void invalidate() {
    invalidation++;
    changed();
  }

  void changed() {
    generation++;
    if (!disposed) notifyListeners();
  }

  Future<Map<String, dynamic>?> call(String? project, Object operation) async {
    if (busy) return null;
    busy = true;
    error = null;
    changed();
    try {
      final response = await request({
        'SkillRequest': {'project_id': project, 'operation': operation},
      });
      final result = response['SkillResponse'];
      if (result == 'Accepted') return {'Accepted': true};
      if (result is! Map) {
        throw const FormatException(
          'This runtime cannot serve skills. Restart with a matching runtime.',
        );
      }
      if (result['Error'] case final Map failure) {
        throw FormatException(failure['message'] as String);
      }
      return Map<String, dynamic>.from(result);
    } on Object catch (e) {
      error = '$e';
      return null;
    } finally {
      busy = false;
      changed();
    }
  }

  Future<List<Map<String, dynamic>>> list(String project) async {
    final entries = <Map<String, dynamic>>[];
    int? offset = 0;
    do {
      final result = await call(project, {
        'List': {
          'offset': offset,
          'limit': 100,
          'source': null,
          'refresh': true,
        },
      });
      if (result == null) break;
      final page = result['Entries'] as Map;
      entries.addAll(
        (page['entries'] as List).map(
          (e) => Map<String, dynamic>.from(e as Map),
        ),
      );
      final errors = (page['errors'] as List).cast<String>();
      if (errors.isNotEmpty) error = errors.join('\n');
      offset = page['next_offset'] as int?;
    } while (offset != null && entries.length < 4096);
    return entries;
  }

  static Map<String, dynamic> binding(Map<String, dynamic> entry) => {
    'identity': entry['identity'],
    'name': entry['name'],
    'path': entry['path'],
    'content_hash': entry['content_hash'],
    'revision': entry['revision'],
    'origin': 'User',
    'reason': null,
    'created_at': DateTime.now().toUtc().toIso8601String(),
  };
  static bool usable(Map entry) =>
      entry['enabled'] == true &&
      entry['recognized'] == true &&
      entry['validation_error'] == null &&
      (entry['missing_dependencies'] as List? ?? []).isEmpty;
  @override
  void dispose() {
    disposed = true;
    super.dispose();
  }
}
