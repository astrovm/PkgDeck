// SPDX-License-Identifier: GPL-2.0-or-later
// Read-only APT metadata bridge. Kept separate from the portable Rust
// frontends.
#include <algorithm>
#include <apt-pkg/cachefile.h>
#include <apt-pkg/configuration.h>
#include <apt-pkg/depcache.h>
#include <apt-pkg/error.h>
#include <apt-pkg/init.h>
#include <apt-pkg/pkgrecords.h>
#include <apt-pkg/pkgsystem.h>
#include <apt-pkg/policy.h>
#include <apt-pkg/version.h>
#include <cctype>
#include <climits>
#include <clocale>
#include <cwchar>
#include <cwctype>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

static std::string json(const std::string &s) {
  std::string out = "\"";
  const char *hex = "0123456789abcdef";
  for (unsigned char c : s) {
    if (c == '"' || c == '\\') {
      out += '\\';
      out += c;
    } else if (c < 32) {
      out += "\\u00";
      out += hex[c >> 4];
      out += hex[c & 15];
    } else
      out += c;
  }
  return out + '"';
}
static std::string lower(const std::string &s) {
  std::string folded;
  std::mbstate_t input{}, output{};
  for (size_t i = 0; i < s.size();) {
    wchar_t c;
    auto count = std::mbrtowc(&c, s.data() + i, s.size() - i, &input);
    if (count == size_t(-1) || count == size_t(-2) || count == 0) {
      folded += s[i++];
      input = {};
      continue;
    }
    i += count;
    c = std::towlower(c);
    if (c == L'ß') {
      folded += "ss";
      continue;
    }
    if (c == L'ς')
      c = L'σ';
    char bytes[MB_LEN_MAX];
    auto length = std::wcrtomb(bytes, c, &output);
    if (length != size_t(-1))
      folded.append(bytes, length);
  }
  return folded;
}
int main(int argc, char **argv) {
  if (argc != 4) {
    std::cerr << "Usage: pkgdeck-apt-query detect|search|installed|details "
                 "query architecture\n";
    return 2;
  }
  const std::string mode = argv[1], query = argv[2], arch = argv[3];
  // The helper is single-threaded; text is UTF-8 regardless of host LANG.
  if (!std::setlocale(LC_CTYPE, "C.UTF-8")) {
    std::cerr << "C.UTF-8 locale is unavailable\n";
    return 1;
  }
  const auto needle = lower(query);
  if (mode != "detect" && mode != "search" && mode != "installed" &&
      mode != "details")
    return 2;
  if (!pkgInitConfig(*_config) || !pkgInitSystem(*_config, _system)) {
    _error->DumpErrors();
    return 1;
  }
  if (mode == "detect") {
    std::cout << "[]\n";
    return 0;
  }
  // Never publish cache files, including when invoked by a privileged caller.
  _config->Set("Dir::Cache::pkgcache", "");
  _config->Set("Dir::Cache::srcpkgcache", "");
  pkgCacheFile file;
  if (!file.ReadOnlyOpen()) {
    _error->DumpErrors();
    return 1;
  }
  auto &cache = *file.GetPkgCache();
  auto &policy = *file.GetPolicy();
  pkgRecords records(cache);
  std::vector<std::pair<pkgCache::PkgIterator, pkgCache::VerIterator>> entries;
  for (auto item = cache.PkgBegin(); !item.end(); ++item) {
    auto installed = item.CurrentVer();
    auto candidate = policy.GetCandidateVer(item);
    auto version = candidate.end() ? installed : candidate;
    if (version.end() || (mode == "installed" && installed.end()))
      continue;
    const std::string name = item.Name(), architecture = version.Arch();
    if (mode == "details" && (name != query || architecture != arch))
      continue;
    if (version.FileList().end())
      continue;
    entries.emplace_back(item, version);
  }
  // Cache hash order would repeatedly seek backwards through compressed
  // indexes. Read each package file in offset order, making a full search a
  // linear scan.
  const auto position = [](const auto &entry) {
    auto record = entry.second.FileList();
    return std::make_pair(record.File()->ID, record->Offset);
  };
  std::sort(entries.begin(), entries.end(), [&](const auto &a, const auto &b) {
    return position(a) < position(b);
  });
  bool first = true;
  std::cout << '[';
  for (const auto &[item, version] : entries) {
    auto installed = item.CurrentVer();
    auto candidate = policy.GetCandidateVer(item);
    const std::string name = item.Name(), architecture = version.Arch();
    auto &record = records.Lookup(version.FileList());
    const std::string summary = record.ShortDesc();
    if (mode == "search" &&
        lower(name + " " + summary).find(needle) == std::string::npos)
      continue;
    const bool upgradable =
        !installed.end() && !candidate.end() &&
        cache.VS->CmpVersion(candidate.VerStr(), installed.VerStr()) > 0 &&
        item->SelectedState != pkgCache::State::Hold &&
        !file.GetDepCache()->PhasingApplied(item);
    if (!first)
      std::cout << ',';
    first = false;
    std::cout << "{\"package\":{\"id\":{\"backend\":\"apt\",\"name\":"
              << json(name) << ",\"architecture\":" << json(architecture)
              << ",\"scope\":\"system\"},\"display_name\":" << json(name)
              << ",\"summary\":" << json(summary) << ",\"installed_version\":"
              << (installed.end() ? "null" : json(installed.VerStr()))
              << ",\"candidate_version\":"
              << (candidate.end() ? "null" : json(candidate.VerStr()))
              << ",\"update\":"
              << json(upgradable        ? "available"
                      : installed.end() ? "unknown"
                                        : "current")
              << "},\"description\":" << json(record.LongDesc())
              << ",\"homepage\":"
              << (record.Homepage().empty() ? "null" : json(record.Homepage()))
              << ",\"dependencies\":[";
    auto depends = record.RecordField("Depends");
    if (!depends.empty())
      std::cout << json(depends);
    std::cout << "]}";
  }
  std::cout << "]\n";
  if (_error->PendingError()) {
    _error->DumpErrors();
    return 1;
  }
}
