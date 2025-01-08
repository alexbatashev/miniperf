#include "llvm/Pass.h"
#include "llvm/Analysis/LoopInfo.h"
#include "llvm/Analysis/ScalarEvolution.h"
#include "llvm/Config/llvm-config.h"
#include "llvm/IR/Constants.h"
#include "llvm/IR/DerivedTypes.h"
#include "llvm/IR/Dominators.h"
#include "llvm/IR/Function.h"
#include "llvm/IR/GlobalValue.h"
#include "llvm/IR/IRBuilder.h"
#include "llvm/IR/Intrinsics.h"
#include "llvm/IR/Module.h"
#include "llvm/IR/PassManager.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Passes/PassPlugin.h"
#include "llvm/Support/Alignment.h"
#include "llvm/Support/raw_ostream.h"

using namespace llvm;

namespace {

struct MiniperfInstr : PassInfoMixin<MiniperfInstr> {
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &FAM) {
    auto &LoopInfo = FAM.getResult<LoopAnalysis>(F);
    auto &DT = FAM.getResult<DominatorTreeAnalysis>(F);

    IRBuilder<> Builder(F.getContext());

    auto LoopInfoTy = StructType::create(
        F.getContext(),
        {Type::getInt32Ty(F.getContext()), PointerType::get(F.getContext(), 0)},
        "LoopInfo");

    auto LoopStatsTy = StructType::create(F.getContext(),
                                          {
                                              // Trip count
                                              Type::getInt64Ty(F.getContext()),
                                              // Bytes load
                                              Type::getInt64Ty(F.getContext()),
                                              // Bytes store
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar int ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar float ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar double ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector int ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector float ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector double ops
                                              Type::getInt64Ty(F.getContext()),
                                          },
                                          "LoopStats");

    for (auto Loop : LoopInfo) {
      if (Loop->getParentLoop())
        continue;

      if (!Loop->getLoopPreheader()) {
        errs() << "Found a loop without a preheader at " << Loop->getLocStr()
               << ":" << Loop->getLocRange().getStart()->getLine()
               << ". Skipping.\n";
        continue;
      }

      if (!Loop->getExitBlock()) {
        errs() << "Found a loop without an exit block at " << Loop->getLocStr()
               << ":" << Loop->getLocRange().getStart()->getLine()
               << ". Skipping.\n";
        continue;
      }

      // We can support this case but later
      if (!DT.dominates(Loop->getLoopPreheader(), Loop->getExitBlock()))
        continue;

      auto *NotifyBegin = F.getParent()->getFunction(
          "mperf_roofline_internal_notify_loop_begin");
      if (!NotifyBegin) {
        auto FuncTy =
            FunctionType::get(PointerType::get(F.getContext(), 0),
                              {PointerType::get(F.getContext(), 0)}, 0);
        NotifyBegin = Function::Create(
            FuncTy, llvm::GlobalValue::ExternalLinkage,
            "mperf_roofline_internal_notify_loop_begin", F.getParent());
      }

      auto *NotifyEnd =
          F.getParent()->getFunction("mperf_roofline_internal_notify_loop_end");
      if (!NotifyEnd) {
        auto FuncTy = FunctionType::get(Type::getVoidTy(F.getContext()),
                                        {PointerType::get(F.getContext(), 0),
                                         PointerType::get(F.getContext(), 0)},
                                        0);
        NotifyEnd = Function::Create(FuncTy, llvm::GlobalValue::ExternalLinkage,
                                     "mperf_roofline_internal_notify_loop_end",
                                     F.getParent());
      }

      Builder.SetInsertPoint(Loop->getLoopPreheader()->getFirstInsertionPt());

      // Create necessary data structures
      Value *StatsMem =
          Builder.CreateAlloca(LoopStatsTy, nullptr, "loop_stats");
      Builder.CreateMemSet(StatsMem,
                           ConstantInt::get(Type::getInt8Ty(F.getContext()), 0),
                           8 * 9, MaybeAlign());

      // Notify loop begin
      Value *Filename = Builder.CreateGlobalString(
          Loop->getLocStr().substr(0, Loop->getLocStr().find(":")));

      Value *InfoMem = Builder.CreateAlloca(LoopInfoTy);

      Value *LineNoPtr = Builder.CreateConstGEP2_32(LoopInfoTy, InfoMem, 0, 0);
      Value *FilenamePtr =
          Builder.CreateConstGEP2_32(LoopInfoTy, InfoMem, 0, 1);

      Builder.CreateStore(Filename, FilenamePtr);
      Builder.CreateStore(ConstantInt::get(Type::getInt32Ty(F.getContext()),
                                           Loop->getStartLoc().getLine()),
                          LineNoPtr);
      Value *LoopHandle = Builder.CreateCall(NotifyBegin, {InfoMem});

      // Notify loop end
      Builder.SetInsertPoint(Loop->getExitBlock()->getFirstInsertionPt());
      Builder.CreateCall(NotifyEnd, {LoopHandle, StatsMem});

      auto UpdateStats = [&](uint64_t Counter, size_t Idx) {
        if (Counter == 0)
          return;

        Value *Ptr =
            Builder.CreateConstInBoundsGEP2_32(LoopStatsTy, StatsMem, 0, Idx);
        Value *Old = Builder.CreateLoad(Type::getInt64Ty(F.getContext()), Ptr);
        Value *New = Builder.CreateAdd(
            Old, ConstantInt::get(Type::getInt64Ty(F.getContext()), Counter));
        Builder.CreateStore(New, Ptr);
      };

      auto ProcessBlock = [&](BasicBlock *BB) {
        uint64_t BytesLoad = 0;
        uint64_t BytesStore = 0;
        uint64_t ScalarIntOps = 0;
        uint64_t ScalarFloatOps = 0;
        uint64_t ScalarDoubleOps = 0;
        uint64_t VectorIntOps = 0;
        uint64_t VectorFloatOps = 0;
        uint64_t VectorDoubleOps = 0;

        auto DL = F.getParent()->getDataLayout();

        for (auto &&I : *BB) {
          switch (I.getOpcode()) {
          case Instruction::Load:
            BytesLoad += DL.getTypeAllocSize(I.getType());
            break;
          case Instruction::Store:
            BytesStore += DL.getTypeAllocSize(I.getOperand(0)->getType());
            break;
          case Instruction::Add:
          case Instruction::Sub:
          case Instruction::Shl:
          case Instruction::Mul:
          case Instruction::CompareUsingScalarTypes:
            if (I.getType()->isVectorTy()) {
              VectorIntOps += 1;
            } else {
              ScalarIntOps += 1;
            }
            break;
          case Instruction::FAdd:
          case Instruction::FMul:
          case Instruction::FSub:
          case Instruction::FDiv:
          case Instruction::FRem:
          case Instruction::FCmp:
            if (I.getType()->isVectorTy()) {
              auto ElementTy = cast<VectorType>(I.getType())->getElementType();
              if (ElementTy->isFloatTy()) {
                VectorFloatOps += 1;
              } else {
                // FIXME this could actually be half or bfloat
                VectorDoubleOps += 1;
              }
            } else if (I.getType()->isFloatTy()) {
              ScalarFloatOps += 1;
            } else {
              ScalarFloatOps += 1;
            }
            break;
          case Instruction::Call: {
            auto &Call = cast<CallInst>(I);
            if (!isa<IntrinsicInst>(Call))
              break;
            switch (Call.getIntrinsicID()) {
            case Intrinsic::fma:
              if (I.getType()->isVectorTy()) {
                auto ElementTy =
                    cast<VectorType>(I.getType())->getElementType();
                if (ElementTy->isFloatTy()) {
                  VectorFloatOps += 2;
                } else {
                  // FIXME this could actually be half or bfloat
                  VectorDoubleOps += 2;
                }
              } else if (I.getType()->isFloatTy()) {
                ScalarFloatOps += 2;
              } else {
                ScalarFloatOps += 2;
              }
              break;
            case Intrinsic::minnum:
            case Intrinsic::minimum:
            case Intrinsic::maxnum:
            case Intrinsic::maximum:
              if (I.getType()->isVectorTy()) {
                auto ElementTy =
                    cast<VectorType>(I.getType())->getElementType();
                if (ElementTy->isFloatTy()) {
                  VectorFloatOps += 1;
                } else {
                  // FIXME this could actually be half or bfloat
                  VectorDoubleOps += 1;
                }
              } else if (I.getType()->isFloatTy()) {
                ScalarFloatOps += 1;
              } else {
                ScalarFloatOps += 1;
              }
              break;
            }
          }
          }
        }
        Builder.SetInsertPoint(BB->getTerminator());
        UpdateStats(BytesLoad, 1);
        UpdateStats(BytesStore, 2);
        UpdateStats(ScalarIntOps, 3);
        UpdateStats(ScalarFloatOps, 4);
        UpdateStats(ScalarDoubleOps, 5);
        UpdateStats(VectorIntOps, 6);
        UpdateStats(VectorFloatOps, 7);
        UpdateStats(VectorDoubleOps, 8);
      };

      for (auto *BB : Loop->getBlocks())
        ProcessBlock(BB);
    }

    return PreservedAnalyses::none();
  }
};

} // namespace

llvm::PassPluginLibraryInfo getMiniperfPluginInfo() {
  return {LLVM_PLUGIN_API_VERSION, "miniperf", LLVM_VERSION_STRING,
          [](PassBuilder &PB) {
            PB.registerOptimizerLastEPCallback([](llvm::ModulePassManager &PM,
                                                  OptimizationLevel Level
#if LLVM_VERSION_MAJOR >= 20
                                                  ,
                                                  ThinOrFullLTOPhase Phase
#endif
                                               ) {
              PM.addPass(createModuleToFunctionPassAdaptor(MiniperfInstr()));
            });
          }};
}

extern "C" LLVM_ATTRIBUTE_WEAK ::llvm::PassPluginLibraryInfo
llvmGetPassPluginInfo() {
  return getMiniperfPluginInfo();
}
